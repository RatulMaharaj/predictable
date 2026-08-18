//! Errors raised while constructing or decoding IR values.
//!
//! These are *structural* errors — a string that is not a legal dtype, a `pir.json` document
//! whose `format` is not `pir/1`. Model-level diagnostics (`E0xxx`, `W0xxx`) belong to the
//! checker (T06) and the diagnostics registry (T03), not here.

use thiserror::Error;

/// A structural IR error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IrError {
    #[error("`{0}` is not a dtype; expected one of f64, i64, bool, date, str, enum(<Name>)")]
    DType(String),

    #[error("`{0}` is not a unit; expected one of none, money, rate(annual|monthly|period), prob, count, years, months, factor")]
    Unit(String),

    #[error("`{0}` is not a table `on_missing` policy; expected error, default(<lit>) or interpolate(<key>)")]
    OnMissing(String),

    #[error("`{0}` is not a valid ExprPath segment")]
    ExprPathSegment(String),

    #[error("an ExprPath must start with `expr` or `init`, found `{0}`")]
    ExprPathRoot(String),

    #[error("unsupported IR format `{found}`; this build reads `{expected}`")]
    Format { found: String, expected: String },

    #[error("this pir.json document matches no known file kind: expected one of the keys `module`, `product`, `run`, `assumption_set`")]
    UnknownFileKind,

    #[error("json error: {0}")]
    Json(String),
}

impl IrError {
    pub(crate) fn dtype(s: &str) -> Self {
        IrError::DType(s.to_string())
    }

    pub(crate) fn unit(s: &str) -> Self {
        IrError::Unit(s.to_string())
    }
}

impl From<serde_json::Error> for IrError {
    fn from(e: serde_json::Error) -> Self {
        IrError::Json(e.to_string())
    }
}
