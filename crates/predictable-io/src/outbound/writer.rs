//! The Parquet/Arrow results writer (`04-verify.md` §2).
//!
//! ## The one invariant
//!
//! Rows are emitted in **`(chunk_idx, offset)` order**. `chunk_idx` is the runner's chunk number
//! and `offset` is the modelpoint's position inside that chunk, so the pair is a total order over
//! the portfolio that does not depend on how many threads ran. The writer *enforces* it rather
//! than trusting it: chunks must arrive with `chunk_idx` ascending from `0` with no gaps, and
//! offsets must strictly ascend within a chunk. A runner that completes chunk 7 before chunk 5
//! must buffer; it may not hand 7 to this writer first, because "same inputs, same bytes" is the
//! property the whole verification loop rests on.
//!
//! Within one modelpoint the writer preserves the order the caller supplies. The runner emits
//! `(component, t)` in schema order, ascending `t`.
//!
//! ## Bytes, not just values
//!
//! Row order is necessary for determinism but not sufficient: the *physical* Parquet layout has
//! to be a function of the row sequence too, or two runs that agree on every number still
//! disagree on `results_digest`. Two rules buy that here:
//!
//! 1. Chunk arrival never reaches Parquet. Rows are buffered and handed to the Arrow writer in
//!    **exact** `batch_rows` slices, so `--chunk-size 1` and `--chunk-size 4096` produce the same
//!    sequence of batches — only the final short batch differs, and only in that it is last.
//! 2. Every writer knob that could move a page or row-group boundary is pinned to a row count
//!    rather than a byte budget (`max_row_group_size`, `data_page_row_count_limit`), and
//!    `created_by` is a constant.
//!
//! A trapped modelpoint under `--continue-on-trap` simply contributes no [`ModelpointRows`]: no
//! null rows, ever (IR §9.3.1).

use std::io::Write;
use std::sync::Arc;

use arrow_array::builder::{
    BooleanBuilder, Float64Builder, Int32Builder, Int64Builder, Int8Builder, StringBuilder,
    StringDictionaryBuilder, UInt32Builder,
};
use arrow_array::types::Int32Type;
use arrow_array::{ArrayRef, RecordBatch};
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use serde::{Deserialize, Serialize};

use crate::outbound::error::{IoOutError, Result};
use crate::outbound::hash::HashingWriter;
use crate::outbound::schema::{results_arrow_schema, ResultsSchemaDoc, ValueColumn};

/// A single cell of the long format: one component at one `t` for one modelpoint.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    /// Qualified `<module_path>.<name>`; must be in the schema document.
    pub component: String,
    /// `-1` for `Scalar`/`PerMP`, `>= 0` for `Series`.
    pub t: i32,
    pub value: Value,
}

impl Cell {
    pub fn f64(component: impl Into<String>, t: i32, v: f64) -> Cell {
        Cell {
            component: component.into(),
            t,
            value: Value::F64(v),
        }
    }

    pub fn i64(component: impl Into<String>, t: i32, v: i64) -> Cell {
        Cell {
            component: component.into(),
            t,
            value: Value::I64(v),
        }
    }

    pub fn bool(component: impl Into<String>, t: i32, v: bool) -> Cell {
        Cell {
            component: component.into(),
            t,
            value: Value::Bool(v),
        }
    }

    pub fn str(component: impl Into<String>, t: i32, v: impl Into<String>) -> Cell {
        Cell {
            component: component.into(),
            t,
            value: Value::Str(v.into()),
        }
    }
}

/// A runtime value. There are no nulls (IR §2.11) — the nullable `value_*` columns express
/// dtype selection only.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    F64(f64),
    I64(i64),
    Bool(bool),
    /// `str`, `enum` (variant name) and `date` (ISO-8601).
    Str(String),
}

impl Value {
    fn kind_name(&self) -> &'static str {
        match self {
            Value::F64(_) => "f64",
            Value::I64(_) => "i64",
            Value::Bool(_) => "bool",
            Value::Str(_) => "str",
        }
    }
}

/// Every cell of one modelpoint, at its `offset` inside the chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelpointRows {
    /// Position within the chunk. Strictly ascending within a chunk.
    pub offset: u32,
    /// The modelpoint's key field value.
    pub mp_key: String,
    /// 0-based row index in the modelpoint file — stable across runs and chunk sizes.
    pub mp_row: u32,
    pub cells: Vec<Cell>,
}

/// One chunk's worth of results, as the runner produces it.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultsChunk {
    pub chunk_idx: u64,
    pub modelpoints: Vec<ModelpointRows>,
}

impl ResultsChunk {
    pub fn new(chunk_idx: u64) -> ResultsChunk {
        ResultsChunk {
            chunk_idx,
            modelpoints: Vec::new(),
        }
    }

    pub fn push(&mut self, mp: ModelpointRows) -> &mut Self {
        self.modelpoints.push(mp);
        self
    }
}

/// Page compression. Snappy is the default because it is pure Rust — no C toolchain, and so no
/// way for a vendored codec build to become part of the reproducibility story.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Codec {
    #[default]
    Snappy,
    /// What the determinism corpus uses, so a byte comparison is over the data and nothing else.
    Uncompressed,
}

/// Rows per data page. A row count, not a byte budget, so pages fall in the same places
/// whatever the values happen to compress to.
const DATA_PAGE_ROWS: usize = 8_192;

/// Dictionary page ceiling. Overflow behaviour (fall back to plain encoding) is a function of
/// the row sequence alone, which is exactly what determinism needs.
const DICTIONARY_PAGE_BYTES: usize = 1024 * 1024;

/// Writer knobs. None of these can change a number; all of them can change file size.
#[derive(Debug, Clone, PartialEq)]
pub struct WriterOptions {
    /// Rows per Arrow batch / Parquet row group flush.
    pub batch_rows: usize,
    pub compression: Codec,
}

impl Default for WriterOptions {
    fn default() -> Self {
        WriterOptions {
            batch_rows: 65_536,
            compression: Codec::default(),
        }
    }
}

impl WriterOptions {
    fn properties(&self) -> Result<WriterProperties> {
        let compression = match self.compression {
            Codec::Uncompressed => Compression::UNCOMPRESSED,
            Codec::Snappy => Compression::SNAPPY,
        };
        if self.batch_rows == 0 {
            return Err(IoOutError::BadBatchRows);
        }
        Ok(WriterProperties::builder()
            .set_compression(compression)
            // Deterministic bytes: no created_by drift between builds.
            .set_created_by("predictable".to_string())
            // Deterministic bytes, part 2 — every knob that can move a page or row-group
            // boundary is pinned to a row count, never to an arrival-dependent byte budget.
            // Combined with the writer only ever handing Parquet full `batch_rows` batches
            // (see `flush_full_batches`), the physical layout is a pure function of the row
            // sequence, so `--chunk-size` cannot change a single byte.
            .set_writer_version(parquet::file::properties::WriterVersion::PARQUET_1_0)
            .set_max_row_group_size(self.batch_rows)
            .set_write_batch_size(1024)
            .set_data_page_row_count_limit(DATA_PAGE_ROWS)
            .set_data_page_size_limit(usize::MAX)
            .set_dictionary_page_size_limit(DICTIONARY_PAGE_BYTES)
            .build())
    }
}

/// What the manifest needs to know about the result set once it is closed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultsSummary {
    pub rows: u64,
    pub components: usize,
    pub component_set_digest: String,
    /// `sha256:…` over the bytes actually written.
    pub digest: String,
    pub bytes: u64,
    /// Distinct modelpoints that contributed at least one row.
    pub modelpoints: u64,
}

/// Streaming writer for `results.parquet`.
#[derive(Debug)]
pub struct ResultsWriter<W: Write + Send> {
    schema: ResultsSchemaDoc,
    options: WriterOptions,
    inner: ArrowWriter<HashingWriter<W>>,
    /// Rows accepted but not yet handed to Parquet. Drained in **exact** `batch_rows` slices, so
    /// the batch boundaries depend on the row count only — never on how the runner chunked the
    /// portfolio. This is what makes `run(C = 1)` and `run(C = 4096)` byte-identical.
    pending: Vec<Row>,
    next_chunk: u64,
    rows: u64,
    modelpoints: u64,
}

/// One buffered long-format row, held until a full batch exists.
#[derive(Debug, Clone)]
struct Row {
    mp_key: Arc<str>,
    mp_row: u32,
    component: Arc<str>,
    stage: i8,
    t: i32,
    value: Value,
}

struct Builders {
    mp_key: StringBuilder,
    mp_row: UInt32Builder,
    component: StringDictionaryBuilder<Int32Type>,
    stage: Int8Builder,
    t: Int32Builder,
    value: Float64Builder,
    value_i: Int64Builder,
    value_b: BooleanBuilder,
    value_s: StringDictionaryBuilder<Int32Type>,
}

// `StringDictionaryBuilder`'s type parameter is not `Debug`, so the derive cannot be used here.
impl std::fmt::Debug for Builders {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Builders { .. }")
    }
}

impl Builders {
    fn new() -> Builders {
        Builders {
            mp_key: StringBuilder::new(),
            mp_row: UInt32Builder::new(),
            component: StringDictionaryBuilder::new(),
            stage: Int8Builder::new(),
            t: Int32Builder::new(),
            value: Float64Builder::new(),
            value_i: Int64Builder::new(),
            value_b: BooleanBuilder::new(),
            value_s: StringDictionaryBuilder::new(),
        }
    }

    fn finish(&mut self) -> Vec<ArrayRef> {
        vec![
            Arc::new(self.mp_key.finish()) as ArrayRef,
            Arc::new(self.mp_row.finish()),
            Arc::new(self.component.finish()),
            Arc::new(self.stage.finish()),
            Arc::new(self.t.finish()),
            Arc::new(self.value.finish()),
            Arc::new(self.value_i.finish()),
            Arc::new(self.value_b.finish()),
            Arc::new(self.value_s.finish()),
        ]
    }
}

impl<W: Write + Send> ResultsWriter<W> {
    /// Open a writer over `sink`. The schema document fixes the emitted component set; a row
    /// naming anything else is an error, not a silently added column value.
    pub fn new(sink: W, schema: ResultsSchemaDoc, options: WriterOptions) -> Result<Self> {
        let props = options.properties()?;
        let inner = ArrowWriter::try_new(
            HashingWriter::new(sink),
            results_arrow_schema(),
            Some(props),
        )?;
        Ok(ResultsWriter {
            schema,
            options,
            inner,
            pending: Vec::new(),
            next_chunk: 0,
            rows: 0,
            modelpoints: 0,
        })
    }

    /// The schema document this writer validates against.
    pub fn schema(&self) -> &ResultsSchemaDoc {
        &self.schema
    }

    /// Append one chunk. Fails if the chunk is out of order, if a modelpoint's offset does not
    /// strictly ascend, or if any cell names an unknown component or the wrong dtype.
    pub fn write_chunk(&mut self, chunk: &ResultsChunk) -> Result<()> {
        if chunk.chunk_idx != self.next_chunk {
            return Err(IoOutError::ChunkOutOfOrder {
                expected: self.next_chunk,
                got: chunk.chunk_idx,
            });
        }
        let mut previous: Option<u32> = None;
        for mp in &chunk.modelpoints {
            if let Some(prev) = previous {
                if mp.offset <= prev {
                    return Err(IoOutError::OffsetOutOfOrder {
                        chunk_idx: chunk.chunk_idx,
                        previous: prev,
                        got: mp.offset,
                    });
                }
            }
            previous = Some(mp.offset);
            self.append_modelpoint(mp)?;
        }
        self.next_chunk += 1;
        self.flush_full_batches()?;
        Ok(())
    }

    fn append_modelpoint(&mut self, mp: &ModelpointRows) -> Result<()> {
        if !mp.cells.is_empty() {
            self.modelpoints += 1;
        }
        let mp_key: Arc<str> = Arc::from(mp.mp_key.as_str());
        for cell in &mp.cells {
            let desc = self.schema.require(&cell.component)?;
            desc.check_t(cell.t)?;
            let column = desc.value_column();
            let expected = matches!(
                (&cell.value, column),
                (Value::F64(_), ValueColumn::F64)
                    | (Value::I64(_), ValueColumn::I64)
                    | (Value::Bool(_), ValueColumn::Bool)
                    | (Value::Str(_), ValueColumn::Str)
            );
            if !expected {
                return Err(IoOutError::DTypeMismatch {
                    component: desc.id.clone(),
                    dtype: desc.dtype.to_string(),
                    found: cell.value.kind_name(),
                });
            }
            self.pending.push(Row {
                mp_key: Arc::clone(&mp_key),
                mp_row: mp.mp_row,
                component: Arc::from(cell.component.as_str()),
                stage: desc.stage,
                t: cell.t,
                value: cell.value.clone(),
            });
            self.rows += 1;
        }
        Ok(())
    }

    /// Hand Parquet every complete `batch_rows` batch that is now available, and keep the
    /// remainder buffered. Never writes a short batch — that is [`finish`](Self::finish)'s job.
    fn flush_full_batches(&mut self) -> Result<()> {
        let n = self.options.batch_rows;
        while self.pending.len() >= n {
            let rest = self.pending.split_off(n);
            let batch = std::mem::replace(&mut self.pending, rest);
            self.write_rows(&batch)?;
        }
        Ok(())
    }

    fn write_rows(&mut self, rows: &[Row]) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let mut b = Builders::new();
        for row in rows {
            b.mp_key.append_value(&*row.mp_key);
            b.mp_row.append_value(row.mp_row);
            b.component.append_value(&*row.component);
            b.stage.append_value(row.stage);
            b.t.append_value(row.t);
            match &row.value {
                Value::F64(v) => {
                    b.value.append_value(*v);
                    b.value_i.append_null();
                    b.value_b.append_null();
                    b.value_s.append_null();
                }
                Value::I64(v) => {
                    b.value.append_null();
                    b.value_i.append_value(*v);
                    b.value_b.append_null();
                    b.value_s.append_null();
                }
                Value::Bool(v) => {
                    b.value.append_null();
                    b.value_i.append_null();
                    b.value_b.append_value(*v);
                    b.value_s.append_null();
                }
                Value::Str(v) => {
                    b.value.append_null();
                    b.value_i.append_null();
                    b.value_b.append_null();
                    b.value_s.append_value(v);
                }
            }
        }
        let batch = RecordBatch::try_new(results_arrow_schema(), b.finish())?;
        self.inner.write(&batch)?;
        Ok(())
    }

    /// Flush, close the Parquet footer and report what the manifest needs.
    pub fn finish(mut self) -> Result<ResultsSummary> {
        self.flush_full_batches()?;
        let tail = std::mem::take(&mut self.pending);
        self.write_rows(&tail)?;
        let sink = self.inner.into_inner()?;
        let (digest, bytes) = sink.finish();
        Ok(ResultsSummary {
            rows: self.rows,
            components: self.schema.len(),
            component_set_digest: self.schema.component_set_digest.clone(),
            digest,
            bytes,
            modelpoints: self.modelpoints,
        })
    }
}
