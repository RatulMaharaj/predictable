//! # `predictable-io::outbound` — outbound run IO (T14)
//!
//! Everything a run *writes*: the long-format results schema, the Parquet/Arrow results writer,
//! the aggregate/solve/table side files, and `manifest.json`. Normative source:
//! `docs/design/04-verify.md` §2 and §6, plus the IR rulings Q12 (lineage), Q13 (table copies)
//! and Q15 (`f64` only).
//!
//! ## The four properties this crate exists to preserve
//!
//! 1. **Long format.** One row per `(modelpoint, component, t)`. It is the only shape that diffs
//!    cleanly when two runs emit different component sets, which is what lets `predictable diff`
//!    say *where* two runs part company.
//! 2. **`(chunk_idx, offset)` order.** Rows leave in a total order that does not depend on thread
//!    scheduling, and [`ResultsWriter`] enforces it rather than assuming it.
//! 3. **`f64` only** (Q15). `value` is Parquet `double`, unconditionally. `storage_precision`
//!    exists as a key so a consumer never has to assume, and `"f32"` is refused (`E0108`).
//! 4. **A result set without a manifest is not evidence.** [`Manifest`] carries the six digests
//!    that make a run reproducible, plus `lineage` (Q12) and the table copies (Q13) that let an
//!    archived run still explain a lookup.
//!
//! ## A whole run, end to end
//!
//! ```no_run
//! use predictable_io::outbound::*;
//! use predictable_ir::{Component, DType, Expr, Kind, Shape};
//!
//! # fn main() -> std::result::Result<(), IoOutError> {
//! let mut bel = Component::derived("bel", DType::F64, Shape::Series, Expr::f64(0.0));
//! bel.kind = Kind::Output;
//! let schema = ResultsSchemaDoc::new(
//!     "outputs",
//!     vec![ComponentDescriptor::from_ir("term_assurance", &bel)],
//! )?;
//!
//! let run = RunDir::create("run")?;
//! let mut w = run.results_writer(schema, WriterOptions::default())?;
//!
//! let mut chunk = ResultsChunk::new(0);
//! chunk.push(ModelpointRows {
//!     offset: 0,
//!     mp_key: "POL0001".into(),
//!     mp_row: 0,
//!     cells: vec![Cell::f64("term_assurance.bel", 0, 1234.5)],
//! });
//! w.write_chunk(&chunk)?;
//! let summary = w.finish()?;
//! assert_eq!(summary.rows, 1);
//! # Ok(())
//! # }
//! ```

#![allow(missing_docs)]

pub mod error;
pub mod hash;
pub mod manifest;
pub mod run_dir;
pub mod schema;
pub mod sidecar;
pub mod writer;

pub use error::{IoOutError, Result};
pub use manifest::{
    Environment, Execution, Inputs, Lineage, Manifest, ModelInputs, ModelpointInputs, Outcome,
    Provenance, ResultsRef, RunConfigInfo, TableInput, VariedInput, Versions,
};
pub use run_dir::RunDir;
pub use schema::{
    results_arrow_schema, ColumnSpec, ComponentDescriptor, ResultsSchemaDoc, ValueColumn, FORMAT,
};
pub use sidecar::{AggregateRow, ArtefactStats, ColumnData, SolveRow, TableCopy};
pub use writer::{
    Cell, Codec, ModelpointRows, ResultsChunk, ResultsSummary, ResultsWriter, Value, WriterOptions,
};

/// Canonical JSON for every `pvf/1` artefact: keys in the order the struct declares them (which
/// is the order `04-verify.md` prints them), two-space indent, LF endings, exactly one trailing
/// newline.
///
/// Serialisation goes straight from the struct rather than through `serde_json::Value`, because
/// `Value`'s map would sort the keys alphabetically and the spec's order is normative.
pub(crate) fn canonical_json<T: serde::Serialize>(value: &T) -> Result<String> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    Ok(text)
}
