//! `predictable-tables` — how a declared table becomes a compiled lookup.
//!
//! A table in predictable is a **declared input** with typed, ordered keys, not
//! a file path buried in a formula (`01-ir.md` §2.9). This crate owns the three
//! steps between that declaration and the kernel's inner loop:
//!
//! 1. **Resolve.** The engine never opens a file; it calls a
//!    [`TableResolver`] ([`FsResolver`], [`MapResolver`], [`InlineResolver`] —
//!    §2.9.1). `source` is relative to the declaring `.pir` and sandboxed to the
//!    project root.
//! 2. **Verify.** Whatever the resolver returned is hashed and compared with the
//!    declared `digest` ([`verify_digest`]). **The digest, not the path, is the
//!    table's identity**, which is what makes swapping a `file:` resolver for a
//!    `resource:` one safe: the two runs either agree on the bytes or are told
//!    they do not. A mismatch is refused unless the run passes
//!    `--allow-table-drift` ([`LoadOptions::allow_table_drift`]).
//! 3. **Compile.** Typed columns become a per-key index and a composed row index
//!    (`03-engine.md` §6): dense vector, perfect hash, sorted + binary search,
//!    `lerp`, and a row-major cartesian dense array while the key product is
//!    under 2²⁴.
//!
//! ```
//! use predictable_ir::{DType, KeyPolicy, OnMissing, TableDecl, TableKey, TableSource, TableValue, Unit};
//! use predictable_tables::{load_bytes, KeyArg, LoadOptions, Outcome, TableBytes, TableFormat};
//!
//! let decl = TableDecl {
//!     name: "mortality".into(),
//!     keys: vec![
//!         TableKey { name: "age".into(), dtype: DType::I64, policy: KeyPolicy::Clamp },
//!         TableKey { name: "smoker".into(), dtype: DType::Bool, policy: KeyPolicy::Exact },
//!     ],
//!     values: vec![TableValue { name: "qx".into(), dtype: DType::F64, unit: Unit::default() }],
//!     on_missing: OnMissing::Error,
//!     source: TableSource::File("tables/mortality.csv".into()),
//!     digest: None,
//!     rows: None,
//!     doc: None,
//! };
//!
//! let csv = "age,smoker,qx\n40,false,0.001\n40,true,0.003\n41,false,0.0012\n41,true,0.0035\n";
//! let bytes = TableBytes { bytes: csv.into(), origin: "tables/mortality.csv".into(), format: TableFormat::Csv };
//! let table = load_bytes(&decl, bytes, &LoadOptions::default()).unwrap();
//!
//! // An exact hit.
//! assert_eq!(table.lookup_f64(&[KeyArg::Int(41), KeyArg::Bool(true)], 0), Outcome::Hit(0.0035));
//! // `age` clamps, so age 99 reads the top of the table rather than missing.
//! assert_eq!(table.lookup_f64(&[KeyArg::Int(99), KeyArg::Bool(true)], 0), Outcome::Hit(0.0035));
//! // 2 ages × 2 smoker values = 4 cells, fully populated.
//! assert_eq!(table.stats().index, "cartesian dense array");
//! ```

#![deny(missing_debug_implementations)]

pub mod compile;
pub mod error;
pub mod index;
pub mod parse;
pub mod resolver;

use std::path::Path;

use predictable_diagnostics::{Diagnostic, Severity};
use predictable_ir::TableDecl;

pub use compile::{
    compile, validate, CompiledTable, Miss, Outcome, RowHit, TableIndex, TableStats,
    CARTESIAN_LIMIT,
};
pub use error::{ResolveError, TableError};
pub use index::{KeyArg, KeyIndex, KeyIndexKind, KeyResolve, Phf};
pub use parse::{parse, KeyCell, RawTable, ValueColumn};
pub use resolver::{
    canonical_rows_text, project_root, FsResolver, InlineResolver, MapResolver, SchemeResolver,
    TableBytes, TableFormat, TableResolver,
};

/// Run-level knobs that change what loading accepts.
#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// `--allow-table-drift`: accept content whose digest differs from the
    /// declaration's. The drift is still recorded on the compiled table so the
    /// manifest can say so (`01-ir.md` §9.4.1).
    pub allow_table_drift: bool,
}

/// The outcome of hashing what a resolver returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestCheck {
    /// `sha256:…` over the bytes actually returned.
    pub actual: String,
    /// The declared digest, if the declaration pins one.
    pub declared: Option<String>,
    /// True when the two differ and the run allowed it.
    pub drifted: bool,
}

/// Hash the resolved bytes and compare them with the declaration (§2.9.1).
///
/// A declaration with no `digest` is not an error here — it is unpinned, and the
/// actual digest is reported so a manifest or `predictable fmt` can record it.
pub fn verify_digest(
    decl: &TableDecl,
    bytes: &TableBytes,
    options: &LoadOptions,
) -> Result<DigestCheck, TableError> {
    let actual = predictable_fmt::digest::digest_bytes(&bytes.bytes);
    let declared = decl.digest.clone();
    let drifted = match &declared {
        Some(want) if want != &actual => {
            if !options.allow_table_drift {
                return Err(TableError::DigestMismatch {
                    table: decl.name.clone(),
                    declared: want.clone(),
                    actual,
                    origin: bytes.origin.clone(),
                });
            }
            true
        }
        _ => false,
    };
    Ok(DigestCheck {
        actual,
        declared,
        drifted,
    })
}

/// Resolve → verify → parse → compile, the whole pipeline for one table.
pub fn load(
    decl: &TableDecl,
    base: &Path,
    resolver: &dyn TableResolver,
    options: &LoadOptions,
) -> Result<CompiledTable, TableError> {
    let bytes = resolver.resolve(decl, base)?;
    load_bytes(decl, bytes, options)
}

/// The half of [`load`] after resolution — useful when the bytes already exist
/// (a hosted run, a test, or `predictable export` re-compiling its own pack).
pub fn load_bytes(
    decl: &TableDecl,
    bytes: TableBytes,
    options: &LoadOptions,
) -> Result<CompiledTable, TableError> {
    compile::validate(decl)?;
    let check = verify_digest(decl, &bytes, options)?;
    let raw = parse::parse(decl, &bytes)?;
    compile::compile(decl, raw, check.actual, bytes.origin, check.drifted)
}

/// Load every table a module declares, in declaration order.
pub fn load_all(
    tables: &[TableDecl],
    base: &Path,
    resolver: &dyn TableResolver,
    options: &LoadOptions,
) -> Result<Vec<CompiledTable>, TableError> {
    tables
        .iter()
        .map(|decl| load(decl, base, resolver, options))
        .collect()
}

/// Render the two load failures that have registered diagnostic codes.
///
/// `E0801` covers a `source` that escapes the project root (§2.9.1); everything
/// else in [`TableError`] is a load failure the runner reports directly, because
/// the registry (`predictable-diagnostics`) is another task's file and codes are
/// not invented here.
pub fn diagnostic(error: &TableError) -> Option<Diagnostic> {
    match error {
        TableError::Resolve(
            ResolveError::EscapesRoot { .. } | ResolveError::AbsolutePath { .. },
        ) => Some(Diagnostic::new("E0801", error.to_string()).severity(Severity::Error)),
        _ => None,
    }
}

/// The `E0902` trap a `Lookup` miss becomes under `on_missing = "error"`
/// (`01-ir.md` §9.3.1, `trap = "lookup_miss"`).
pub fn lookup_miss_trap(miss: &Miss) -> Diagnostic {
    Diagnostic::new("E0902", miss.to_string())
        .severity(Severity::Error)
        .note("`on_missing = \"error\"` is the default; set `on_missing = \"default(<lit>)\"` on the table if a miss is expected")
}
