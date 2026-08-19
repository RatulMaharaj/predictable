//! Inbound half of `predictable-io`: reading modelpoints into typed, dense chunk columns.
//!
//! One trait, three implementations:
//!
//! | source | notes |
//! |---|---|
//! | [`ParquetSource`] | preferred; the [`MpSchema`] projection is pushed into the Parquet reader, so a model touching 7 of an MPF's 60 columns reads 7 |
//! | [`CsvSource`] | header-validated, typed parse, empty cell = null |
//! | [`ArrowSource`] | a `RecordBatchReader` handed over from Python |
//!
//! Three invariants, all from the specs:
//!
//! 1. **The schema comes from the IR** (§2.10). It is validated against the file's own schema
//!    *before row 1* — a missing required column names the file, the column and the referencing
//!    components.
//! 2. **No nulls survive the boundary** (§2.11, ruling Q3). A missing or null cell in an optional
//!    field becomes the declared `default`; a null in a required field is an error. Lanes are
//!    dense, with no validity bitmap.
//! 3. **Chunk boundaries are a function of `C` and the row count only** (`03-engine.md` §6). The
//!    physical batching of the underlying file is invisible: `next_chunk(n, ..)` yields exactly
//!    `n` rows until the last chunk.

mod arrow_source;
mod csv_source;
mod parquet_source;

pub use arrow_source::{ArrowSource, BatchReader};
pub use csv_source::CsvSource;
pub use parquet_source::ParquetSource;

use predictable_ir::DType;

use crate::error::{IoError, Lint};
use crate::schema::{DefaultValue, MpField, MpSchema};

/// A stream of modelpoints, chunked deterministically.
pub trait ModelpointSource: Send {
    /// The load plan this source was opened against.
    fn schema(&self) -> &MpSchema;

    /// Fill `out` with up to `n` rows, returning the number of rows written. `0` means the stream
    /// is exhausted. `out` is cleared first; its column layout must match [`Self::schema`].
    fn next_chunk(&mut self, n: usize, out: &mut ChunkColumns) -> Result<usize, IoError>;

    /// Non-fatal observations gathered while opening the file (unknown extra columns, pruned
    /// columns).
    fn lints(&self) -> &[Lint];
}

/// One dense, typed column of a chunk. There is no validity bitmap: nulls were eliminated at load.
#[derive(Debug, Clone, PartialEq)]
pub enum MpColumn {
    /// `dtype = "f64"`.
    F64(Vec<f64>),
    /// `dtype = "i64"`.
    I64(Vec<i64>),
    /// `dtype = "bool"`.
    Bool(Vec<bool>),
    /// `dtype = "date"`, as days since the Unix epoch.
    Date(Vec<i32>),
    /// `dtype = "str"`.
    Str(Vec<String>),
    /// `dtype = "enum(X)"`, as dictionary codes into [`MpField::variants`].
    Enum(Vec<u32>),
}

impl MpColumn {
    fn empty_for(dtype: &DType) -> MpColumn {
        match dtype {
            DType::F64 => MpColumn::F64(Vec::new()),
            DType::I64 => MpColumn::I64(Vec::new()),
            DType::Bool => MpColumn::Bool(Vec::new()),
            DType::Date => MpColumn::Date(Vec::new()),
            DType::Str => MpColumn::Str(Vec::new()),
            DType::Enum(_) => MpColumn::Enum(Vec::new()),
        }
    }

    /// The number of rows held.
    pub fn len(&self) -> usize {
        match self {
            MpColumn::F64(v) => v.len(),
            MpColumn::I64(v) => v.len(),
            MpColumn::Bool(v) => v.len(),
            MpColumn::Date(v) => v.len(),
            MpColumn::Str(v) => v.len(),
            MpColumn::Enum(v) => v.len(),
        }
    }

    /// True when the column holds no rows.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn clear(&mut self) {
        match self {
            MpColumn::F64(v) => v.clear(),
            MpColumn::I64(v) => v.clear(),
            MpColumn::Bool(v) => v.clear(),
            MpColumn::Date(v) => v.clear(),
            MpColumn::Str(v) => v.clear(),
            MpColumn::Enum(v) => v.clear(),
        }
    }

    /// The `f64` lane, if this is one.
    pub fn as_f64(&self) -> Option<&[f64]> {
        match self {
            MpColumn::F64(v) => Some(v),
            _ => None,
        }
    }

    /// The `i64` lane, if this is one.
    pub fn as_i64(&self) -> Option<&[i64]> {
        match self {
            MpColumn::I64(v) => Some(v),
            _ => None,
        }
    }

    /// The `bool` lane, if this is one.
    pub fn as_bool(&self) -> Option<&[bool]> {
        match self {
            MpColumn::Bool(v) => Some(v),
            _ => None,
        }
    }

    /// The `str` lane, if this is one.
    pub fn as_str(&self) -> Option<&[String]> {
        match self {
            MpColumn::Str(v) => Some(v),
            _ => None,
        }
    }

    /// The enum dictionary-code lane, if this is one.
    pub fn as_enum(&self) -> Option<&[u32]> {
        match self {
            MpColumn::Enum(v) => Some(v),
            _ => None,
        }
    }

    /// The `date` lane (days since the Unix epoch), if this is one.
    pub fn as_date(&self) -> Option<&[i32]> {
        match self {
            MpColumn::Date(v) => Some(v),
            _ => None,
        }
    }

    fn push_default(&mut self, default: &DefaultValue) {
        match (self, default) {
            (MpColumn::F64(v), DefaultValue::F64(x)) => v.push(*x),
            (MpColumn::I64(v), DefaultValue::I64(x)) => v.push(*x),
            (MpColumn::Bool(v), DefaultValue::Bool(x)) => v.push(*x),
            (MpColumn::Date(v), DefaultValue::Date(x)) => v.push(*x),
            (MpColumn::Str(v), DefaultValue::Str(x)) => v.push(x.clone()),
            (MpColumn::Enum(v), DefaultValue::Enum(x)) => v.push(*x),
            _ => unreachable!("default was decoded against this field's dtype"),
        }
    }
}

/// A chunk of modelpoints: one dense column per field of the [`MpSchema`], plus the opt-in
/// presence lanes of §2.11.
///
/// The buffer is reused across chunks — [`ModelpointSource::next_chunk`] clears it rather than
/// reallocating — which is what keeps the loader off the allocator in the steady state.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkColumns {
    columns: Vec<MpColumn>,
    presence: Vec<Option<Vec<bool>>>,
    names: Vec<String>,
    rows: usize,
    chunk_idx: u64,
    first_row: u64,
}

impl ChunkColumns {
    /// An empty buffer laid out for `schema`.
    pub fn for_schema(schema: &MpSchema) -> ChunkColumns {
        ChunkColumns {
            columns: schema
                .fields()
                .iter()
                .map(|f| MpColumn::empty_for(&f.dtype))
                .collect(),
            presence: schema
                .fields()
                .iter()
                .map(|f| if f.presence { Some(Vec::new()) } else { None })
                .collect(),
            names: schema.fields().iter().map(|f| f.name.clone()).collect(),
            rows: 0,
            chunk_idx: 0,
            first_row: 0,
        }
    }

    /// The rows held.
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// True when the chunk holds no rows.
    pub fn is_empty(&self) -> bool {
        self.rows == 0
    }

    /// The chunk's position in the file, counting from 0. Results carry it (IR §9.1).
    pub fn chunk_idx(&self) -> u64 {
        self.chunk_idx
    }

    /// The 0-based file row index of this chunk's first row.
    pub fn first_row(&self) -> u64 {
        self.first_row
    }

    /// The columns, in [`MpSchema`] order.
    pub fn columns(&self) -> &[MpColumn] {
        &self.columns
    }

    /// The column named `name`.
    pub fn column(&self, name: &str) -> Option<&MpColumn> {
        self.names
            .iter()
            .position(|n| n == name)
            .map(|i| &self.columns[i])
    }

    /// The presence lane for `name` — `Some` only for optional fields the model calls
    /// `is_null`/`coalesce` on. `true` means the cell was present in the file.
    pub fn presence(&self, name: &str) -> Option<&[bool]> {
        self.names
            .iter()
            .position(|n| n == name)
            .and_then(|i| self.presence[i].as_deref())
    }

    fn reset(&mut self, chunk_idx: u64, first_row: u64) {
        for column in &mut self.columns {
            column.clear();
        }
        for lane in self.presence.iter_mut().flatten() {
            lane.clear();
        }
        self.rows = 0;
        self.chunk_idx = chunk_idx;
        self.first_row = first_row;
    }

    fn finish(&mut self, rows: usize) {
        self.rows = rows;
    }

    fn column_mut(&mut self, i: usize) -> &mut MpColumn {
        &mut self.columns[i]
    }

    fn note_presence(&mut self, i: usize, present: bool) {
        if let Some(lane) = self.presence[i].as_mut() {
            lane.push(present);
        }
    }

    /// Append the field's default and record an absent presence bit (§2.11).
    fn push_absent(&mut self, i: usize, field: &MpField) -> Result<(), IoError> {
        let default = field
            .default
            .as_ref()
            .ok_or_else(|| IoError::OptionalWithoutDefault {
                column: field.name.clone(),
            })?;
        self.columns[i].push_default(default);
        self.note_presence(i, false);
        Ok(())
    }
}

/// Check that the buffer was laid out for this schema. Cheap, and it turns a caller mistake into a
/// message instead of a silently wrong column.
fn check_layout(schema: &MpSchema, out: &ChunkColumns, file: &str) -> Result<(), IoError> {
    if out.names.len() == schema.len()
        && out
            .names
            .iter()
            .zip(schema.fields())
            .all(|(n, f)| n == &f.name)
    {
        return Ok(());
    }
    Err(IoError::Backend {
        file: file.to_string(),
        message: "the ChunkColumns buffer was built for a different schema".to_string(),
    })
}
