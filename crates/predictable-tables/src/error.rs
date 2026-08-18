//! Errors raised while resolving, verifying and compiling a table.
//!
//! Nothing here is a *checker* diagnostic: the checker (T06) validates the
//! declaration, this crate validates the **content** the resolver returned. Two
//! of these failures do have registered diagnostic codes and can be rendered as
//! such — see [`crate::diagnostic`].

use std::fmt;
use std::path::PathBuf;

/// Why a [`crate::TableResolver`] could not produce bytes.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    /// `source` resolved outside the project root (`01-ir.md` §2.9.1, `E0801`).
    #[error("table `{table}` source `{spec}` escapes the project root `{root}`")]
    EscapesRoot {
        table: String,
        spec: String,
        root: PathBuf,
    },
    /// An absolute `source` path. Absolute paths are `E0801` (§2.9.1).
    #[error("table `{table}` source `{spec}` is an absolute path; sources are relative to the declaring .pir file")]
    AbsolutePath { table: String, spec: String },
    /// The resolver was handed a `source` scheme it does not serve.
    #[error("{resolver} cannot serve table `{table}` with source `{spec}`")]
    WrongScheme {
        resolver: &'static str,
        table: String,
        spec: String,
    },
    /// A `resource:` name the host map does not carry.
    #[error("table `{table}`: no resource named `{name}` was supplied by the host")]
    UnknownResource { table: String, name: String },
    /// `source = "inline"` with no `rows` array.
    #[error("table `{table}` declares `source = \"inline\"` but carries no `rows`")]
    NoInlineRows { table: String },
    /// The bytes could not be read.
    #[error("table `{table}`: reading `{path}` failed: {source}")]
    Io {
        table: String,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Where a row/cell problem is, for the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellAt {
    /// 1-based data row (the header is not a data row).
    pub row: usize,
    /// Column name as declared in the IR.
    pub column: String,
}

impl fmt::Display for CellAt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "row {}, column `{}`", self.row, self.column)
    }
}

/// Why a table's content could not be turned into a compiled lookup structure.
#[derive(Debug, thiserror::Error)]
pub enum TableError {
    #[error(transparent)]
    Resolve(#[from] ResolveError),

    /// The bytes the resolver returned do not hash to the declared digest.
    ///
    /// The digest, not the path, is the table's identity (§2.9.1), so this is
    /// refused unless the run passes `--allow-table-drift`.
    #[error(
        "table `{table}` content does not match its declared digest\n  \
         declared: {declared}\n  actual:   {actual}\n  origin:   {origin}\n  \
         re-run with `--allow-table-drift` to accept the new content, or update `digest` in the declaration"
    )]
    DigestMismatch {
        table: String,
        declared: String,
        actual: String,
        origin: String,
    },

    /// A format this build cannot parse (Parquet tables are read through
    /// `predictable-io`, not here).
    #[error("table `{table}`: {format} sources are not readable by predictable-tables (origin `{origin}`)")]
    UnsupportedFormat {
        table: String,
        format: &'static str,
        origin: String,
    },

    #[error("table `{table}`: the source is not valid UTF-8")]
    NotUtf8 { table: String },

    #[error("table `{table}`: the source has no header row")]
    NoHeader { table: String },

    /// A declared key/value column the file does not carry.
    #[error("table `{table}`: column `{column}` is declared but the source has only [{present}]")]
    MissingColumn {
        table: String,
        column: String,
        present: String,
    },

    #[error("table `{table}`: {at} has {got} fields, expected {want}")]
    RowArity {
        table: String,
        at: usize,
        got: usize,
        want: usize,
    },

    #[error("table `{table}`: {at}: `{text}` is not a valid {dtype}")]
    BadCell {
        table: String,
        at: CellAt,
        text: String,
        dtype: String,
    },

    #[error("table `{table}`: the key tuple ({tuple}) appears on rows {first} and {second}")]
    DuplicateKey {
        table: String,
        tuple: String,
        first: usize,
        second: usize,
    },

    #[error("table `{table}`: the source has no rows")]
    Empty { table: String },

    /// Enum and string keys are unordered: `exact` only (§2.9; the checker
    /// reports this as `E0305`, this is the loader's own guard).
    #[error("table `{table}`: key `{key}` has dtype `{dtype}`, which supports `policy = \"exact\"` only, not `{policy}`")]
    UnorderedKeyPolicy {
        table: String,
        key: String,
        dtype: String,
        policy: &'static str,
    },

    /// No float keys in the index path (`03-engine.md` §6).
    #[error("table `{table}`: key `{key}` is `f64` with `policy = \"exact\"`; float equality is not an index. Use `clamp`, `step` or `interpolate`")]
    FloatExactKey { table: String, key: String },

    #[error("table `{table}`: keys {keys} both interpolate; at most one key may interpolate")]
    MultipleInterpolatedKeys { table: String, keys: String },

    #[error("table `{table}`: `on_missing = \"interpolate({key})\"` names `{key}`, which is not a key of this table")]
    UnknownInterpolationKey { table: String, key: String },

    #[error("table `{table}`: `on_missing = \"default({value})\"` does not fit value column `{column}` of dtype `{dtype}`")]
    DefaultDtype {
        table: String,
        value: String,
        column: String,
        dtype: String,
    },

    #[error("table `{table}`: key `{key}` interpolates, but value column `{column}` is `{dtype}`; only `f64` values can be interpolated")]
    NonNumericInterpolation {
        table: String,
        key: String,
        column: String,
        dtype: String,
    },
}
