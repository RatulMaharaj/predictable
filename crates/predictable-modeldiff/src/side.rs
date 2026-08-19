//! One side of a diff: the modules, assumption values and table rows that make
//! up a model as it was at one point in time.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use predictable_ir::{Component, LitValue, Module, TableDecl, TableSource};
use predictable_syntax::ast::DocumentKind;

/// A model as the diff sees it.
///
/// The diff is a pure function of this value, so a caller can build one from
/// files, from a database, or by hand in a test — the CLI has no privileged
/// path into the comparison.
#[derive(Debug, Clone, Default)]
pub struct ModelSide {
    /// How this side is named in the report, e.g. a path or `"before"`.
    pub label: String,
    /// The model modules, in file order.
    pub modules: Vec<Module>,
    /// Assumption-set values, name → value, merged across assumption files.
    pub assumption_values: BTreeMap<String, LitValue>,
    /// Table name → the rows of its CSV, header excluded, cells as written.
    /// Absent for a table whose bytes were not available.
    pub table_rows: BTreeMap<String, Vec<Vec<String>>>,
    /// Files that failed to parse, if any — reported, never guessed around.
    pub unparsed: Vec<String>,
}

impl ModelSide {
    /// An empty side with a label.
    pub fn new(label: impl Into<String>) -> ModelSide {
        ModelSide {
            label: label.into(),
            ..ModelSide::default()
        }
    }

    /// Every component of every module, in declaration order.
    pub fn components(&self) -> impl Iterator<Item = &Component> {
        self.modules.iter().flat_map(|m| m.components.iter())
    }

    /// Component by unqualified name. Names are globally unique within a
    /// product (§2.2), so the first match is the only match.
    pub fn component(&self, name: &str) -> Option<&Component> {
        self.components().find(|c| c.name == name)
    }

    /// Every table declaration, in declaration order.
    pub fn tables(&self) -> impl Iterator<Item = &TableDecl> {
        self.modules.iter().flat_map(|m| m.tables.iter())
    }

    /// The declared assumptions, name → dtype/unit/shape rendered for report.
    pub fn assumption_decls(&self) -> BTreeMap<String, String> {
        self.modules
            .iter()
            .flat_map(|m| m.assumptions.iter())
            .map(|a| {
                (
                    a.name.clone(),
                    format!("{} {} {}", a.dtype, a.unit, a.shape),
                )
            })
            .collect()
    }

    /// Parse `.pir` text into a side. Module files become modules; assumption
    /// sets contribute their values; products and run files are ignored,
    /// because neither is part of the model's *structure*.
    pub fn from_inputs<'a, I>(label: impl Into<String>, inputs: I) -> ModelSide
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        let mut side = ModelSide::new(label);
        for (name, text) in inputs {
            let mut sources = predictable_syntax::SourceMap::new();
            let parsed =
                predictable_syntax::parse(&mut sources, name.to_string(), text.to_string());
            if parsed.has_errors() {
                side.unparsed.push(name.to_string());
                continue;
            }
            match parsed.document.kind() {
                DocumentKind::Module => {
                    side.modules
                        .push(predictable_check::lower::module(&parsed.document));
                }
                DocumentKind::AssumptionSet => {
                    for (key, value) in &parsed.document.assumption_values {
                        if let Some(lit) = raw_literal(value) {
                            side.assumption_values.insert(key.value.clone(), lit);
                        }
                    }
                }
                _ => {}
            }
        }
        side
    }

    /// Read `.pir` files from disk, then read the CSV of every file-backed
    /// table so the report can diff rows rather than only digests (§11.3).
    ///
    /// A table whose CSV is missing or unreadable is not an error here: the
    /// digest comparison still works, and the row diff is simply absent.
    pub fn load(label: impl Into<String>, files: &[PathBuf]) -> Result<ModelSide, String> {
        let mut texts = Vec::with_capacity(files.len());
        for path in files {
            let text =
                std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            texts.push((path.clone(), text));
        }
        let mut side = ModelSide::from_inputs(
            label,
            texts
                .iter()
                .map(|(p, t)| (p.to_str().unwrap_or_default(), t.as_str())),
        );
        // Table sources are relative to the declaring file's directory (§2.9.1).
        for (path, _) in &texts {
            let base = path.parent().unwrap_or_else(|| Path::new("."));
            let names: Vec<(String, String)> = side
                .tables()
                .filter_map(|t| match &t.source {
                    TableSource::File(p) => Some((t.name.clone(), p.clone())),
                    _ => None,
                })
                .collect();
            for (name, rel) in names {
                if side.table_rows.contains_key(&name) {
                    continue;
                }
                if let Some(text) = read_table(base, &rel) {
                    side.table_rows.insert(name, csv_rows(&text));
                }
            }
        }
        // Inline tables carry their rows in the declaration itself.
        let inline: Vec<(String, Vec<Vec<String>>)> = side
            .tables()
            .filter_map(|t| {
                t.rows.as_ref().map(|rows| {
                    (
                        t.name.clone(),
                        rows.iter()
                            .map(|r| r.iter().map(crate::render::render_lit).collect())
                            .collect(),
                    )
                })
            })
            .collect();
        side.table_rows.extend(inline);
        Ok(side)
    }
}

/// Read a table's CSV, looking for it under the declaring file's directory and
/// then under each ancestor.
///
/// `source` is relative to the *project* root (§2.9.1), which the run supplies
/// and a diff has no run to ask. Walking up from the file is the only way to
/// find `tables/mortality.csv` when the declaration lives in `build/`; failing
/// to find it costs the row diff, never the digest comparison, so a wrong guess
/// is impossible — only a missing one.
fn read_table(dir: &Path, rel: &str) -> Option<String> {
    let mut cursor = Some(dir);
    while let Some(base) = cursor {
        if let Ok(text) = std::fs::read_to_string(base.join(rel)) {
            return Some(text);
        }
        cursor = base.parent();
    }
    None
}

/// Split CSV text into rows of trimmed cells, dropping the header line and any
/// blank lines. Quoted commas are not supported here for the same reason the
/// table loader does not need them: key and value cells are numbers, dates and
/// bare identifiers.
fn csv_rows(text: &str) -> Vec<Vec<String>> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .skip(1)
        .map(|l| l.split(',').map(|c| c.trim().to_string()).collect())
        .collect()
}

fn raw_literal(v: &predictable_syntax::raw::Value) -> Option<LitValue> {
    use predictable_syntax::raw::Value;
    Some(match v {
        Value::Str(s) => LitValue::Text(s.clone()),
        Value::Int(i) => LitValue::Int(*i),
        Value::Float(x) => LitValue::Float(*x),
        Value::Bool(b) => LitValue::Bool(*b),
        Value::Date(d) => LitValue::Text(d.clone()),
        _ => return None,
    })
}
