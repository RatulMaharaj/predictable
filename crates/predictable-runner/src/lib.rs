//! # `predictable-runner` — everything around the kernel
//!
//! The kernel ([`predictable_engine`]) projects one chunk of modelpoints and computes numbers.
//! This crate is everything that has to happen around it for a *run* to exist: reading
//! modelpoints, deciding where chunks execute, solving, aggregating, writing the run directory
//! and stamping the manifest. It is deliberately the only crate that knows about threads, the
//! clock and the filesystem at the same time.
//!
//! | Concern | Lives in | Spec |
//! |---|---|---|
//! | Chunk pipeline | [`pipeline`] | `03-engine.md` §5.1 |
//! | Where a chunk runs | [`executor`] | §5.1, §10 |
//! | Cancellation | [`cancel`] | §10, `01-ir.md` §9.3.1 |
//! | `[[solve]]` Brent outer loop | [`solve`], [`solver`] | §5.4, IR §8.4.4 (Q8) |
//! | `[[aggregation]]` monoid fold | [`aggregate`] | IR §8.3 (Q4) |
//! | Run directory + manifest | [`run`] | IR §9.4.1 (Q1, Q12, Q13), `04-verify.md` §6 |
//! | `[run]` file → typed config | [`config`] | IR §8.4.2 |
//!
//! ## The four invariants
//!
//! 1. **Nothing here computes an arithmetic result.** Every number comes out of the kernel. The
//!    runner decides *what* is projected and *what is written*, never *what a value is*.
//! 2. **Order is a function of the data, not of the schedule.** Chunks come back in chunk index
//!    order from every [`executor::ChunkExecutor`], aggregation partials combine in chunk index
//!    order, and results are written in `(chunk_idx, offset)` order — so
//!    `run(threads = 1) ≡ run(threads = 64)` and `run(C = 1) ≡ run(C = 1024)`, bit for bit.
//! 3. **A short result set never looks complete.** A trap under `on_trap = "abort"` writes a
//!    manifest and no results; `continue` drops the modelpoint entirely and says so; a cancelled
//!    run is stamped `"outcome": "cancelled"` with a projected count short of the row count.
//! 4. **A result set without provenance is not evidence.** Every run writes the six digests,
//!    `lineage` (Q12), the solve outcomes (Q8) and — unless suppressed — the table content that
//!    was actually read (Q13).
//!
//! ## A run, end to end
//!
//! ```
//! use std::collections::BTreeMap;
//! use predictable_check::Input;
//! use predictable_plan::{plan_sources, PlanOptions};
//! use predictable_runner::chunk::{Chunk, ChunkColumn};
//! use predictable_runner::executor::{LocalExecutor, SerialExecutor};
//! use predictable_runner::pipeline::{RunInputs, Runner};
//! use predictable_runner::CancelFlag;
//! use predictable_tape::lower_plan;
//!
//! let src = r#"
//! format = "pir/1"
//! module = "term"
//!
//! [timeline]
//! basis = "annual"
//! periods = 2
//! origin = "policy"
//! valuation_date = 2026-06-30
//!
//! [[modelpoint_field]]
//! name = "policy_number"
//! dtype = "str"
//! required = true
//! key = true
//!
//! [[modelpoint_field]]
//! name = "sum_assured"
//! dtype = "f64"
//! unit = "money"
//! required = true
//!
//! [[component]]
//! name = "claims"
//! kind = "Output"
//! dtype = "f64"
//! shape = "Series"
//! unit = "money"
//! timing = "end"
//! expr = "sum_assured * 0.01"
//! "#;
//!
//! let inputs = [Input::new("term.pir", src)];
//! let plan = plan_sources(&inputs, &PlanOptions::default()).unwrap();
//! let tapes = lower_plan(&plan).unwrap();
//! let modules = predictable_plan::lower_modules(&inputs);
//!
//! // A run file with no solves and no aggregations: the plain projection.
//! let run = predictable_runner::minimal_run_file();
//!
//! let runner = Runner::new(RunInputs {
//!     modules: &modules,
//!     plan: &plan,
//!     tapes: &tapes,
//!     tables: vec![],
//!     assumptions: BTreeMap::new(),
//!     run: &run,
//! })
//! .unwrap();
//!
//! let mut columns = BTreeMap::new();
//! columns.insert("sum_assured".to_string(), ChunkColumn::Num(vec![100_000.0, 250_000.0]));
//! columns.insert(
//!     "policy_number".to_string(),
//!     ChunkColumn::Text(vec!["POL1".into(), "POL2".into()]),
//! );
//! let chunks = vec![Chunk {
//!     index: 0,
//!     first_row: 0,
//!     keys: vec!["POL1".into(), "POL2".into()],
//!     columns,
//! }];
//!
//! let cancel = CancelFlag::new();
//! let serial = runner.project(&chunks, &SerialExecutor, &cancel).unwrap();
//! let parallel = runner.project(&chunks, &LocalExecutor::new(4), &cancel).unwrap();
//!
//! let claims = serial.chunks[0].column("term.claims").unwrap();
//! assert_eq!(claims.lane(0), &[1000.0, 1000.0, 1000.0]);
//! // The executor is a scheduling choice, never a numerical one.
//! assert_eq!(
//!     parallel.chunks[0].column("term.claims").unwrap().values,
//!     claims.values
//! );
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(missing_debug_implementations)]

pub mod aggregate;
pub mod cancel;
pub mod chunk;
pub mod config;
pub mod error;
pub mod executor;
pub mod pipeline;
pub mod run;
pub mod solve;
pub mod solver;

pub use aggregate::AggregationStage;
pub use cancel::CancelFlag;
pub use chunk::{load_chunks, Chunk, ChunkColumn};
pub use error::RunError;
pub use executor::{ChunkExecutor, ChunkResult, ChunkWorker, LocalExecutor, SerialExecutor};
pub use pipeline::{Projection, RunInputs, Runner};
pub use run::{execute, write_run, RunReport, RunSpec};
pub use solve::{brent, BrentState, Status};
pub use solver::SolveResult;

/// A `[run]` configuration with every field at its `01-ir.md` §8.4.2 default: emit the outputs,
/// abort on a trap, no solves, no aggregations.
///
/// Real runs come from a `.pir` file through [`config::run_file`]. This exists for the doc
/// example above, for tests, and for an embedding that has no run file at all — a notebook
/// asking "project this model over this data" is a legitimate caller, and it should not have to
/// synthesise TOML to be one.
pub fn minimal_run_file() -> predictable_ir::run::RunFile {
    predictable_ir::run::RunFile {
        format: "pir/1".to_string(),
        run: predictable_ir::run::RunConfig {
            product: String::new(),
            assumptions: None,
            modelpoints: String::new(),
            out: String::new(),
            emit: Default::default(),
            emit_list: Vec::new(),
            retain: Default::default(),
            storage_precision: Default::default(),
            on_trap: Default::default(),
            max_errors: 100,
            allow_table_drift: false,
            sum_kahan: false,
            tables: Default::default(),
            exec: Default::default(),
        },
        solves: Vec::new(),
        aggregations: Vec::new(),
    }
}
