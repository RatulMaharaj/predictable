//! [`ArrowSource`] — a `RecordBatchReader` (typically handed over from Python) read as
//! modelpoints, and the batch decoder the Parquet source reuses.

use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::types::{Date32Type, Float32Type, Float64Type, Int32Type, Int64Type};
use arrow_array::{Array, ArrayRef, RecordBatch, RecordBatchReader};
use arrow_schema::{DataType, Schema};

use crate::error::{IoError, Lint};
use crate::inbound::{check_layout, ChunkColumns, ModelpointSource, MpColumn};
use crate::schema::{MpField, MpSchema};

/// Modelpoints from any Arrow `RecordBatchReader`.
///
/// The reader's own batch sizes are irrelevant: rows are re-chunked to exactly the `n` asked for,
/// so `run(C = 1)` and `run(C = 1024)` see the same rows in the same order.
pub struct ArrowSource {
    schema: MpSchema,
    file: String,
    reader: Box<dyn RecordBatchReader + Send>,
    /// Column index in the incoming batches for each schema field; `None` for an optional field
    /// the file omits entirely (every cell is then the declared default).
    columns: Vec<Option<usize>>,
    pending: Option<(RecordBatch, usize)>,
    lints: Vec<Lint>,
    next_chunk_idx: u64,
    next_row: u64,
    done: bool,
}

impl std::fmt::Debug for ArrowSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArrowSource")
            .field("file", &self.file)
            .field("fields", &self.schema.len())
            .field("next_chunk_idx", &self.next_chunk_idx)
            .finish()
    }
}

impl ArrowSource {
    /// Open a reader against `schema`. The Arrow schema is validated **before the first batch is
    /// pulled**: a missing required column fails here, not on row 40,000.
    pub fn new(
        schema: MpSchema,
        reader: Box<dyn RecordBatchReader + Send>,
        file: impl Into<String>,
    ) -> Result<ArrowSource, IoError> {
        let file = file.into();
        let arrow_schema = reader.schema();
        let (columns, mut lints) = validate(&schema, &arrow_schema, &file)?;
        lints.extend(schema.pruned_lints(&file));
        Ok(ArrowSource {
            schema,
            file,
            reader,
            columns,
            pending: None,
            lints,
            next_chunk_idx: 0,
            next_row: 0,
            done: false,
        })
    }

    fn pull(&mut self) -> Result<bool, IoError> {
        loop {
            match self.reader.next() {
                None => {
                    self.done = true;
                    return Ok(false);
                }
                Some(Err(e)) => {
                    return Err(IoError::Backend {
                        file: self.file.clone(),
                        message: e.to_string(),
                    })
                }
                Some(Ok(batch)) => {
                    if batch.num_rows() > 0 {
                        self.pending = Some((batch, 0));
                        return Ok(true);
                    }
                }
            }
        }
    }
}

impl ModelpointSource for ArrowSource {
    fn schema(&self) -> &MpSchema {
        &self.schema
    }

    fn next_chunk(&mut self, n: usize, out: &mut ChunkColumns) -> Result<usize, IoError> {
        check_layout(&self.schema, out, &self.file)?;
        out.reset(self.next_chunk_idx, self.next_row);
        if n == 0 {
            return Ok(0);
        }
        let mut written = 0usize;
        while written < n {
            if self.pending.is_none() && !self.done && !self.pull()? {
                break;
            }
            let Some((batch, offset)) = self.pending.take() else {
                break;
            };
            let take = (batch.num_rows() - offset).min(n - written);
            append_rows(
                out,
                &self.schema,
                &self.columns,
                &batch,
                offset,
                take,
                &self.file,
                self.next_row + written as u64,
            )?;
            written += take;
            if offset + take < batch.num_rows() {
                self.pending = Some((batch, offset + take));
            }
        }
        out.finish(written);
        if written > 0 {
            self.next_chunk_idx += 1;
            self.next_row += written as u64;
        }
        Ok(written)
    }

    fn lints(&self) -> &[Lint] {
        &self.lints
    }
}

/// Match the IR modelpoint schema against a file's Arrow schema, before row 1.
///
/// Returns the per-field column index (`None` = optional field absent from the file, every cell
/// takes the declared default) and the lints for undeclared extra columns (§2.10: accepted with a
/// lint).
pub(crate) fn validate(
    schema: &MpSchema,
    arrow_schema: &Schema,
    file: &str,
) -> Result<(Vec<Option<usize>>, Vec<Lint>), IoError> {
    let mut columns = Vec::with_capacity(schema.len());
    for field in schema.fields() {
        let matches: Vec<usize> = arrow_schema
            .fields()
            .iter()
            .enumerate()
            .filter(|(_, f)| f.name() == &field.name)
            .map(|(i, _)| i)
            .collect();
        match matches.len() {
            0 => {
                if field.required {
                    return Err(IoError::MissingColumn {
                        file: file.to_string(),
                        column: field.name.clone(),
                        referenced_by: field.referenced_by.clone(),
                    });
                }
                columns.push(None);
            }
            1 => {
                let idx = matches[0];
                let dt = arrow_schema.field(idx).data_type();
                if !compatible(field, dt) {
                    return Err(IoError::TypeMismatch {
                        file: file.to_string(),
                        column: field.name.clone(),
                        expected: field.dtype.clone(),
                        found: dt.to_string(),
                    });
                }
                columns.push(Some(idx));
            }
            _ => {
                return Err(IoError::DuplicateColumn {
                    file: file.to_string(),
                    column: field.name.clone(),
                })
            }
        }
    }

    let lints = arrow_schema
        .fields()
        .iter()
        .filter(|f| schema.field(f.name()).is_none() && !schema.is_pruned(f.name()))
        .map(|f| Lint::UnknownColumn {
            file: file.to_string(),
            column: f.name().clone(),
        })
        .collect();
    Ok((columns, lints))
}

/// Which Arrow physical types can supply an IR dtype. Widening only: never a silent narrowing,
/// never a string-to-number coercion.
fn compatible(field: &MpField, dt: &DataType) -> bool {
    match field.dtype {
        predictable_ir::DType::F64 => matches!(
            dt,
            DataType::Float64 | DataType::Float32 | DataType::Int64 | DataType::Int32
        ),
        predictable_ir::DType::I64 => matches!(dt, DataType::Int64 | DataType::Int32),
        predictable_ir::DType::Bool => matches!(dt, DataType::Boolean),
        predictable_ir::DType::Date => matches!(dt, DataType::Date32),
        predictable_ir::DType::Str | predictable_ir::DType::Enum(_) => {
            matches!(dt, DataType::Utf8 | DataType::LargeUtf8)
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn append_rows(
    out: &mut ChunkColumns,
    schema: &MpSchema,
    columns: &[Option<usize>],
    batch: &RecordBatch,
    offset: usize,
    take: usize,
    file: &str,
    first_file_row: u64,
) -> Result<(), IoError> {
    for (i, field) in schema.fields().iter().enumerate() {
        match columns[i] {
            None => {
                for _ in 0..take {
                    out.push_absent(i, field)?;
                }
            }
            Some(col) => {
                let array = batch.column(col);
                for r in 0..take {
                    let row = offset + r;
                    let file_row = first_file_row + r as u64 + 1;
                    if array.is_null(row) {
                        if field.required {
                            return Err(IoError::NullInRequired {
                                file: file.to_string(),
                                column: field.name.clone(),
                                row: file_row,
                            });
                        }
                        out.push_absent(i, field)?;
                    } else {
                        append_value(out, i, field, array, row, file, file_row)?;
                        out.note_presence(i, true);
                    }
                }
            }
        }
    }
    Ok(())
}

fn append_value(
    out: &mut ChunkColumns,
    i: usize,
    field: &MpField,
    array: &ArrayRef,
    row: usize,
    file: &str,
    file_row: u64,
) -> Result<(), IoError> {
    let dt = array.data_type().clone();
    match out.column_mut(i) {
        MpColumn::F64(v) => v.push(match dt {
            DataType::Float64 => array.as_primitive::<Float64Type>().value(row),
            DataType::Float32 => array.as_primitive::<Float32Type>().value(row) as f64,
            DataType::Int64 => array.as_primitive::<Int64Type>().value(row) as f64,
            _ => array.as_primitive::<Int32Type>().value(row) as f64,
        }),
        MpColumn::I64(v) => v.push(match dt {
            DataType::Int64 => array.as_primitive::<Int64Type>().value(row),
            _ => array.as_primitive::<Int32Type>().value(row) as i64,
        }),
        MpColumn::Bool(v) => v.push(array.as_boolean().value(row)),
        MpColumn::Date(v) => v.push(array.as_primitive::<Date32Type>().value(row)),
        MpColumn::Str(v) => v.push(string_at(array, row).to_string()),
        MpColumn::Enum(v) => {
            let text = string_at(array, row);
            let code = field
                .variants
                .iter()
                .position(|variant| variant == text)
                .ok_or_else(|| IoError::UnknownVariant {
                    file: file.to_string(),
                    column: field.name.clone(),
                    row: file_row,
                    value: text.to_string(),
                    enum_name: field.enum_name().unwrap_or_default().to_string(),
                    expected: field.variants.clone(),
                })?;
            v.push(code as u32);
        }
    }
    Ok(())
}

fn string_at(array: &ArrayRef, row: usize) -> &str {
    match array.data_type() {
        DataType::LargeUtf8 => array.as_string::<i64>().value(row),
        _ => array.as_string::<i32>().value(row),
    }
}

/// A `RecordBatchReader` over an owned batch list, used by tests and by callers that already hold
/// Arrow data in memory (the PyO3 bridge of `03-engine.md` §8).
#[derive(Debug)]
pub struct BatchReader {
    schema: Arc<Schema>,
    batches: std::vec::IntoIter<RecordBatch>,
}

impl BatchReader {
    /// A reader over `batches`, all of which must share `schema`.
    pub fn new(schema: Arc<Schema>, batches: Vec<RecordBatch>) -> BatchReader {
        BatchReader {
            schema,
            batches: batches.into_iter(),
        }
    }
}

impl Iterator for BatchReader {
    type Item = Result<RecordBatch, arrow_schema::ArrowError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.batches.next().map(Ok)
    }
}

impl RecordBatchReader for BatchReader {
    fn schema(&self) -> Arc<Schema> {
        Arc::clone(&self.schema)
    }
}
