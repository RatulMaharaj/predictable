//! The `TableResolver` contract of `01-ir.md` §2.9.1.
//!
//! The engine never opens a file. It hands a [`TableDecl`] and the directory of
//! the declaring `.pir` to a resolver and gets bytes back. Three resolvers are
//! normative for 1.0 and all three live here:
//!
//! | Resolver | Selected by | Behaviour |
//! |---|---|---|
//! | [`FsResolver`] | default, `file` scheme | reads `base.join(source)`, sandboxed to the project root |
//! | [`MapResolver`] | `resource:` scheme | host-supplied `BTreeMap<String, Vec<u8>>`; WASM and hosted runs |
//! | [`InlineResolver`] | `source = "inline"` | reads the `rows` array in the declaration itself |
//!
//! Whatever comes back is hashed and compared to `digest` before compilation
//! ([`crate::verify_digest`]) — the digest, not the path, is the table's identity.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use predictable_ir::{LitValue, TableDecl, TableSource};

use crate::error::ResolveError;

/// How the bytes a resolver returned should be parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableFormat {
    /// Comma-separated, with a header row naming the declared columns.
    Csv,
    /// Apache Parquet. Read through `predictable-io`, not this crate.
    Parquet,
    /// Arrow IPC. Read through `predictable-io`, not this crate.
    Arrow,
    /// The rows carried in the declaration, encoded as canonical text.
    Inline,
}

impl TableFormat {
    /// The name used in error messages.
    pub fn label(self) -> &'static str {
        match self {
            TableFormat::Csv => "CSV",
            TableFormat::Parquet => "Parquet",
            TableFormat::Arrow => "Arrow",
            TableFormat::Inline => "inline",
        }
    }

    /// The format implied by a path's extension. Anything unrecognised is CSV,
    /// which is what an extensionless `tables/sa8990` almost always is.
    pub fn of_path(path: &Path) -> TableFormat {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("parquet") | Some("pq") => TableFormat::Parquet,
            Some("arrow") | Some("feather") | Some("ipc") => TableFormat::Arrow,
            _ => TableFormat::Csv,
        }
    }
}

/// The bytes of one table, plus where they came from.
///
/// `origin` is for humans and manifests: it is the resolved path, the
/// `resource:` name, or `inline`. It never participates in a digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableBytes {
    pub bytes: Vec<u8>,
    pub origin: String,
    pub format: TableFormat,
}

/// The one seam through which table content enters the engine (§2.9.1).
pub trait TableResolver {
    /// `decl` is the table declaration; `base` is the directory of the
    /// declaring `.pir` file.
    fn resolve(&self, decl: &TableDecl, base: &Path) -> Result<TableBytes, ResolveError>;
}

/// Lexically normalise a path: fold `.` away and pop a component for each `..`,
/// without touching the filesystem (the file may legitimately not exist yet,
/// and `canonicalize` would follow symlinks out of the sandbox).
///
/// A leading `..` that cannot be popped is kept, so `../../etc/passwd` stays
/// outside any root and is caught by the sandbox check.
pub(crate) fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if out
                    .components()
                    .next_back()
                    .is_some_and(|c| !matches!(c, Component::ParentDir | Component::RootDir))
                {
                    out.pop();
                } else {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The project root: the nearest ancestor of `base` containing
/// `predictable.toml`, else `base` itself (§2.9.1).
pub fn project_root(base: &Path) -> PathBuf {
    let base = normalise(base);
    let mut cursor: &Path = &base;
    loop {
        if cursor.join("predictable.toml").is_file() {
            return cursor.to_path_buf();
        }
        match cursor.parent() {
            Some(parent) if parent != cursor => cursor = parent,
            _ => return base,
        }
    }
}

/// The default resolver: reads `base.join(source)`, sandboxed to the project root.
#[derive(Debug, Clone, Default)]
pub struct FsResolver {
    /// Pin the sandbox root explicitly. `None` discovers it per call with
    /// [`project_root`].
    root: Option<PathBuf>,
}

impl FsResolver {
    /// Discover the project root per call (the ordinary case).
    pub fn new() -> Self {
        FsResolver { root: None }
    }

    /// Pin the sandbox root — what a host does when it already knows the
    /// project boundary and does not want it re-derived.
    pub fn rooted_at(root: impl Into<PathBuf>) -> Self {
        FsResolver {
            root: Some(normalise(&root.into())),
        }
    }

    /// The path `source` denotes, with the sandbox rule of §2.9.1 applied.
    ///
    /// This is public because `predictable export` and the manifest writer need
    /// the same answer without reading the bytes.
    pub fn path_for(&self, decl: &TableDecl, base: &Path) -> Result<PathBuf, ResolveError> {
        let rel = match &decl.source {
            TableSource::File(p) => p,
            other => {
                return Err(ResolveError::WrongScheme {
                    resolver: "FsResolver",
                    table: decl.name.clone(),
                    spec: other.to_string(),
                })
            }
        };
        let as_path = Path::new(rel);
        if as_path.is_absolute() {
            return Err(ResolveError::AbsolutePath {
                table: decl.name.clone(),
                spec: rel.clone(),
            });
        }
        let root = match &self.root {
            Some(r) => r.clone(),
            None => project_root(base),
        };
        let resolved = normalise(&normalise(base).join(as_path));
        if !resolved.starts_with(&root) {
            return Err(ResolveError::EscapesRoot {
                table: decl.name.clone(),
                spec: rel.clone(),
                root,
            });
        }
        Ok(resolved)
    }
}

impl TableResolver for FsResolver {
    fn resolve(&self, decl: &TableDecl, base: &Path) -> Result<TableBytes, ResolveError> {
        let path = self.path_for(decl, base)?;
        let bytes = std::fs::read(&path).map_err(|source| ResolveError::Io {
            table: decl.name.clone(),
            path: path.clone(),
            source,
        })?;
        Ok(TableBytes {
            bytes,
            origin: path.display().to_string(),
            format: TableFormat::of_path(&path),
        })
    }
}

/// The `resource:` resolver: a host-supplied name → bytes map.
///
/// This is what WASM, hosted runs and `predictable export` use — there is no
/// filesystem in those builds, and `resolve` never touches one here either.
#[derive(Debug, Clone, Default)]
pub struct MapResolver {
    entries: BTreeMap<String, Vec<u8>>,
    formats: BTreeMap<String, TableFormat>,
}

impl MapResolver {
    pub fn new() -> Self {
        MapResolver::default()
    }

    /// Add a resource. The format defaults to CSV, which is what a `resource:`
    /// carrying text almost always is; use [`MapResolver::insert_as`] otherwise.
    pub fn insert(&mut self, name: impl Into<String>, bytes: impl Into<Vec<u8>>) -> &mut Self {
        self.entries.insert(name.into(), bytes.into());
        self
    }

    /// Add a resource with an explicit format.
    pub fn insert_as(
        &mut self,
        name: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
        format: TableFormat,
    ) -> &mut Self {
        let name = name.into();
        self.formats.insert(name.clone(), format);
        self.entries.insert(name, bytes.into());
        self
    }

    /// The resource names available, in sorted order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }
}

impl FromIterator<(String, Vec<u8>)> for MapResolver {
    fn from_iter<I: IntoIterator<Item = (String, Vec<u8>)>>(iter: I) -> Self {
        MapResolver {
            entries: iter.into_iter().collect(),
            formats: BTreeMap::new(),
        }
    }
}

impl TableResolver for MapResolver {
    fn resolve(&self, decl: &TableDecl, _base: &Path) -> Result<TableBytes, ResolveError> {
        let name = match &decl.source {
            TableSource::Resource(name) => name,
            other => {
                return Err(ResolveError::WrongScheme {
                    resolver: "MapResolver",
                    table: decl.name.clone(),
                    spec: other.to_string(),
                })
            }
        };
        let bytes = self
            .entries
            .get(name)
            .ok_or_else(|| ResolveError::UnknownResource {
                table: decl.name.clone(),
                name: name.clone(),
            })?;
        Ok(TableBytes {
            bytes: bytes.clone(),
            origin: format!("resource:{name}"),
            format: self.formats.get(name).copied().unwrap_or(TableFormat::Csv),
        })
    }
}

/// The `source = "inline"` resolver: the rows are in the declaration.
///
/// The bytes it returns are the **canonical text of the `rows` array** — the
/// same text `predictable fmt` writes and the same text
/// `predictable_fmt::digest::inline_rows_digest` hashes, so an inline table's
/// digest is reproducible from the module alone.
#[derive(Debug, Clone, Copy, Default)]
pub struct InlineResolver;

impl TableResolver for InlineResolver {
    fn resolve(&self, decl: &TableDecl, _base: &Path) -> Result<TableBytes, ResolveError> {
        if decl.source != TableSource::Inline {
            return Err(ResolveError::WrongScheme {
                resolver: "InlineResolver",
                table: decl.name.clone(),
                spec: decl.source.to_string(),
            });
        }
        let rows = decl
            .rows
            .as_ref()
            .ok_or_else(|| ResolveError::NoInlineRows {
                table: decl.name.clone(),
            })?;
        Ok(TableBytes {
            bytes: canonical_rows_text(rows).into_bytes(),
            origin: "inline".to_string(),
            format: TableFormat::Inline,
        })
    }
}

/// The canonical text of a `rows = [...]` array — one row per line, `fmt`'s
/// float form, so the digest of an inline table is a pure function of the IR.
pub fn canonical_rows_text(rows: &[Vec<LitValue>]) -> String {
    let mut out = String::from("[\n");
    for row in rows {
        out.push_str("  [");
        for (i, cell) in row.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(&lit_text(cell));
        }
        out.push_str("],\n");
    }
    out.push_str("]\n");
    out
}

pub(crate) fn lit_text(v: &LitValue) -> String {
    match v {
        LitValue::Float(x) => predictable_fmt::format_f64(*x),
        LitValue::Text(s) => format!("\"{s}\""),
        other => other.to_string(),
    }
}

/// A resolver that dispatches on the `source` scheme — the one a runner
/// actually installs, because one model may mix `file:` and `resource:` tables.
#[derive(Debug, Default)]
pub struct SchemeResolver {
    pub fs: FsResolver,
    pub map: MapResolver,
    pub inline: InlineResolver,
}

impl SchemeResolver {
    /// A dispatcher with a filesystem resolver and an empty resource map.
    pub fn new() -> Self {
        SchemeResolver::default()
    }

    /// A dispatcher with no filesystem reachable — the WASM configuration: a
    /// `file:` source will still be attempted and will fail at `read`, which is
    /// the honest outcome, but `resource:` and `inline` work.
    pub fn with_resources(map: MapResolver) -> Self {
        SchemeResolver {
            map,
            ..SchemeResolver::default()
        }
    }
}

impl TableResolver for SchemeResolver {
    fn resolve(&self, decl: &TableDecl, base: &Path) -> Result<TableBytes, ResolveError> {
        match &decl.source {
            TableSource::File(_) => self.fs.resolve(decl, base),
            TableSource::Resource(_) => self.map.resolve(decl, base),
            TableSource::Inline => self.inline.resolve(decl, base),
        }
    }
}
