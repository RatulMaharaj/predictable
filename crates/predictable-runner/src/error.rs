//! Why a run could not proceed.
//!
//! Every variant names the thing the user must change. The runner does not invent diagnostic
//! codes — the registry is `predictable-diagnostics`' file — except where `01-ir.md` already
//! fixed one: `E0902` for a trap envelope (§9.3.1) and `E0903` for a solve that did not
//! converge under `on_not_converged = "error"` (§8.4.4).

use predictable_engine::EngineError;
use predictable_io::IoError;

/// A run-level failure.
#[derive(Debug)]
pub enum RunError {
    /// The modelpoint source failed, or a required column was absent.
    Io(Box<IoError>),
    /// The kernel refused a chunk.
    Engine(EngineError),
    /// Writing the run directory failed.
    Out(Box<predictable_io::outbound::IoOutError>),
    /// A modelpoint column the schema promised was not in the loaded chunk.
    MissingColumn(String),
    /// `key_field` is not a `str` or `i64` column: a results join key must be renderable.
    BadKeyField(String),
    /// `chunk_size = 0`.
    BadChunkSize,
    /// An aggregation reads a component the run did not emit.
    AggregationInputNotEmitted(String),
    /// `op = "weighted_mean"` without a `weight` (`01-ir.md` §8.3).
    AggregationWeightRequired(String),
    /// `emit = "list"` named a component that does not resolve (`E0106`).
    UnknownComponent(String),
    /// A requested `Series` component is retained as a ring, so it cannot be emitted whole.
    /// Plan with `retain = "full"` (`03-engine.md` §4.3).
    NotRetained(String),
    /// A `[[solve]]` names something that cannot be varied or targeted (`01-ir.md` §8.4.4).
    BadSolve {
        /// The `[[solve]]` name.
        solve: String,
        /// What is wrong with it.
        reason: String,
    },
    /// A `str`/`enum` component was emitted: its lane holds a per-engine dictionary code, not
    /// text, and writing the code would put a meaningless integer in the result set.
    TextComponent(String),
    /// `E0903`: a solve did not converge and `on_not_converged = "error"`.
    NotConverged {
        /// The `[[solve]]` name.
        solve: String,
        /// How many modelpoints hit `max_iter`.
        modelpoints: u64,
    },
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::Io(e) => write!(f, "{e}"),
            RunError::Engine(e) => write!(f, "{e}"),
            RunError::Out(e) => write!(f, "{e}"),
            RunError::MissingColumn(c) => write!(f, "modelpoint column `{c}` was not loaded"),
            RunError::BadKeyField(c) => write!(
                f,
                "key field `{c}` is neither `str` nor `i64`; a results join key must be renderable"
            ),
            RunError::BadChunkSize => f.write_str("chunk_size must be at least 1"),
            RunError::AggregationInputNotEmitted(c) => write!(
                f,
                "aggregation input `{c}` is not in the emitted component set"
            ),
            RunError::AggregationWeightRequired(n) => {
                write!(f, "aggregation `{n}`: op = \"weighted_mean\" requires `weight`")
            }
            RunError::UnknownComponent(c) => write!(f, "E0106: `{c}` does not resolve to a component"),
            RunError::NotRetained(c) => write!(
                f,
                "`{c}` is retained as a ring and cannot be emitted; plan the run with retain = \"full\""
            ),
            RunError::TextComponent(c) => write!(
                f,
                "component `{c}` is `str`/`enum`; the runner cannot emit text components in IR 1.0"
            ),
            RunError::BadSolve { solve, reason } => write!(f, "[[solve]] `{solve}`: {reason}"),
            RunError::NotConverged { solve, modelpoints } => write!(
                f,
                "E0903: solve `{solve}` did not converge for {modelpoints} modelpoint(s)"
            ),
        }
    }
}

impl std::error::Error for RunError {}

impl From<IoError> for RunError {
    fn from(e: IoError) -> RunError {
        RunError::Io(Box::new(e))
    }
}

impl From<EngineError> for RunError {
    fn from(e: EngineError) -> RunError {
        RunError::Engine(e)
    }
}

impl From<predictable_io::outbound::IoOutError> for RunError {
    fn from(e: predictable_io::outbound::IoOutError) -> RunError {
        RunError::Out(Box::new(e))
    }
}
