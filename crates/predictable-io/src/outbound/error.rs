//! Errors the outbound IO layer can raise.
//!
//! Every variant names the artefact and the rule that was broken, because a writer that fails
//! silently produces a result set that looks complete and is not (`04-verify.md` §6.3 rule 6).

use thiserror::Error;

/// The result type of every fallible operation in this crate.
pub type Result<T> = std::result::Result<T, IoOutError>;

#[derive(Debug, Error)]
pub enum IoOutError {
    /// Chunks must arrive in `(chunk_idx, offset)` order (`04-verify.md` §2): `chunk_idx`
    /// ascending from 0 with no gaps.
    #[error("results must be written in (chunk_idx, offset) order: expected chunk_idx {expected}, got {got}")]
    ChunkOutOfOrder { expected: u64, got: u64 },

    /// Within a chunk the modelpoint offsets must strictly ascend.
    #[error("results must be written in (chunk_idx, offset) order: chunk {chunk_idx} offset {got} follows offset {previous}")]
    OffsetOutOfOrder {
        chunk_idx: u64,
        previous: u32,
        got: u32,
    },

    /// A row named a component that is not in `results.schema.json`. The emitted set is a
    /// manifest, not a suggestion.
    #[error("component `{0}` is not in the emitted component set")]
    UnknownComponent(String),

    /// Which `value*` column a row lands in is decided by the component's IR `dtype`, never by
    /// inspecting the value (`04-verify.md` §2).
    #[error("component `{component}` has dtype {dtype}, which cannot hold a {found} value")]
    DTypeMismatch {
        component: String,
        dtype: String,
        found: &'static str,
    },

    /// `Scalar` and `PerMP` components carry `t = -1`; `Series` components carry `t >= 0`.
    #[error("component `{component}` is {shape}, so t must be {expected}, got {got}")]
    BadT {
        component: String,
        shape: String,
        expected: &'static str,
        got: i32,
    },

    /// Q15: `storage_precision` accepts `"f64"` only in IR 1.0; `"f32"` is `E0108`.
    #[error("E0108: run.storage_precision = \"f32\" is not supported in IR 1.0; results are written as double")]
    UnsupportedStoragePrecision,

    /// `batch_rows = 0` would make the batch boundary undefined, and with it the file bytes.
    #[error("WriterOptions.batch_rows must be at least 1")]
    BadBatchRows,

    /// Two components in one schema document shared a qualified id.
    #[error("duplicate component id `{0}` in the emitted component set")]
    DuplicateComponent(String),

    #[error("arrow error: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),

    #[error("parquet error: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("io error writing {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

impl IoOutError {
    pub(crate) fn io(path: impl Into<String>, source: std::io::Error) -> IoOutError {
        IoOutError::Io {
            path: path.into(),
            source,
        }
    }
}
