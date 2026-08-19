//! # `predictable-engine` — the kernel
//!
//! The kernel executes a lowered tape ([`predictable_tape`]) against modelpoint
//! data. It is the only thing in the system that computes a number, and it is
//! deliberately the smallest crate that could do so: **no threads, no IO, no
//! clock, no allocator dependence** (`03-engine.md` §1). Parallelism is a
//! partition of the modelpoint set one layer up, in the runner; it is never a
//! change to this crate, which is what makes "the same engine runs in the
//! browser" true rather than aspirational.
//!
//! Four decisions carry the whole design:
//!
//! | Concern | Answer | Spec |
//! |---|---|---|
//! | Data layout | `series_buf[slot][t_ring][c]`, chunks of `C = 1024`, lanes innermost | §4.2 |
//! | Memory | one [`ChunkBuffers`] arena per worker, reused for every chunk, never zeroed | §4.4 |
//! | Errors | per-lane [`TrapFlags`], checked once per period, then a scalar replay | §5.5 |
//! | Reductions | strictly sequential in `t`, never vectorised | `01-ir.md` §9.2 |
//!
//! ## Worked example
//!
//! ```
//! use std::collections::BTreeMap;
//! use predictable_check::Input;
//! use predictable_engine::{ChunkInput, Engine, RunConfig};
//! use predictable_plan::{plan_sources, PlanOptions};
//! use predictable_tape::lower_plan;
//!
//! let src = r#"
//! format = "pir/1"
//! module = "term"
//!
//! [timeline]
//! basis = "annual"
//! periods = 3
//! origin = "policy"
//! valuation_date = 2026-06-30
//!
//! [[modelpoint_field]]
//! name = "sum_assured"
//! dtype = "f64"
//! unit = "money"
//! required = true
//!
//! [[modelpoint_field]]
//! name = "q"
//! dtype = "f64"
//! unit = "prob"
//! required = true
//!
//! [[component]]
//! name = "survivors"
//! kind = "Derived"
//! dtype = "f64"
//! shape = "Series"
//! unit = "count"
//! timing = "start"
//! init = "1.0"
//! expr = "survivors[t-1] * (1 - q)"
//!
//! [[component]]
//! name = "claims"
//! kind = "Output"
//! dtype = "f64"
//! shape = "Series"
//! unit = "money"
//! timing = "end"
//! expr = "survivors * q * sum_assured"
//! "#;
//!
//! let plan = plan_sources(&[Input::new("term.pir", src)], &PlanOptions::default()).unwrap();
//! let tapes = lower_plan(&plan).unwrap();
//! let timeline = predictable_ir::Timeline {
//!     basis: predictable_ir::Basis::Annual,
//!     periods: 3,
//!     origin: predictable_ir::Origin::Policy,
//!     valuation_date: "2026-06-30".to_string(),
//!     year_convention: "act/365".to_string(),
//! };
//!
//! let mut engine = Engine::new(&plan, &tapes, vec![], &timeline, RunConfig::default()).unwrap();
//! let mut bufs = engine.buffers();
//! engine.prepare(&BTreeMap::new(), &mut bufs).unwrap();
//!
//! let mut columns = BTreeMap::new();
//! columns.insert("sum_assured".to_string(), vec![100_000.0, 250_000.0]);
//! columns.insert("q".to_string(), vec![0.01, 0.02]);
//! let chunk = ChunkInput {
//!     index: 0,
//!     keys: vec!["POL1".into(), "POL2".into()],
//!     first_row: 0,
//!     columns,
//! };
//!
//! let out = engine.run_chunk(&mut bufs, &chunk).unwrap();
//! let claims = out.column("term.claims").unwrap();
//! // t = 0: 1.0 * 0.01 * 100_000
//! assert_eq!(claims.lane(0)[0], 1000.0);
//! // t = 1: survivors have run off by one period
//! assert_eq!(claims.lane(0)[1], 0.99 * 0.01 * 100_000.0);
//! ```

#![forbid(unsafe_code)]
#![deny(missing_debug_implementations)]

pub mod buffers;
pub mod dates;
pub mod dict;
mod exec;
pub mod explain;
pub mod explain_text;
pub mod layout;
pub mod levels;
pub mod ops;
mod reduce;
pub mod run;
pub mod terms;
pub mod timeline;
pub mod traps;

pub use buffers::ChunkBuffers;
pub use dict::Dictionary;
pub use exec::{RunState, TableBinding};
pub use explain::{
    expr_text, AggOver, Divergence, ExplainError, ExplainOptions, Explainer, LookupKey, Node,
    NodeKind, Note, Trace, TraceContext,
};
pub use layout::{Layout, Place};
pub use levels::Levels;
pub use run::{ChunkInput, ChunkOutput, Engine, EngineError, OutputColumn, RunConfig};
pub use terms::{AggTrace, Term, DEFAULT_MAX_TERMS};
pub use timeline::Clock;
pub use traps::{TrapFlags, TrapKind, TrapLog, TrapPolicy, TrapReport};

use serde::{Deserialize, Serialize};

/// Version of the on-disk IR schema this engine understands.
///
/// Bumped whenever the serialized model format changes in a way that older
/// engines cannot read. Models carry this so a mismatch is a clear, early error
/// rather than a mysterious evaluation failure.
pub const IR_SCHEMA_VERSION: u32 = 1;

/// Engine build version (mirrors the crate version).
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Placeholder for the root of a compiled model.
///
/// Real field set is specified in `docs/design/01-ir.md`; this stub exists so
/// the workspace compiles, tests run, and the PyO3 crate has something to bind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelIr {
    /// Must equal [`IR_SCHEMA_VERSION`] for this engine to accept the model.
    pub schema_version: u32,
    /// Human-facing model name, used in diagnostics and result metadata.
    pub name: String,
}

impl ModelIr {
    /// Create an empty model at the current schema version.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            schema_version: IR_SCHEMA_VERSION,
            name: name.into(),
        }
    }

    /// Cheap compatibility check performed before any other validation.
    pub fn is_compatible(&self) -> bool {
        self.schema_version == IR_SCHEMA_VERSION
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_model_is_compatible_and_roundtrips() {
        let model = ModelIr::new("term_assurance");
        assert!(model.is_compatible());
        assert_eq!(model.schema_version, IR_SCHEMA_VERSION);

        let json = serde_json::to_string(&model).expect("serialize");
        let back: ModelIr = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(model, back);
    }

    #[test]
    fn reports_a_version() {
        assert!(!version().is_empty());
    }
}
