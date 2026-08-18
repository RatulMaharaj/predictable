//! The three side artefacts of a run directory: `aggregates.parquet` (`04-verify.md` §2,
//! IR §8.3), `solves/<name>.parquet` (IR §8.4.4) and `tables/<name>.parquet` — the table copies
//! Q13 makes mandatory.
//!
//! They share the results writer's two habits: fixed column order, and a digest taken over the
//! bytes actually written.

use std::io::Write;
use std::sync::Arc;

use arrow_array::builder::{
    BooleanBuilder, Float64Builder, Int32Builder, Int64Builder, StringBuilder, UInt32Builder,
};
use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

use crate::outbound::error::{IoOutError, Result};
use crate::outbound::hash::HashingWriter;

fn props() -> WriterProperties {
    WriterProperties::builder()
        .set_created_by("predictable".to_string())
        .build()
}

fn write_batch<W: Write + Send>(
    sink: W,
    schema: Arc<Schema>,
    columns: Vec<ArrayRef>,
) -> Result<(String, u64, u64)> {
    let rows = columns.first().map(|c| c.len()).unwrap_or(0) as u64;
    let batch = RecordBatch::try_new(schema.clone(), columns)?;
    let mut w = ArrowWriter::try_new(HashingWriter::new(sink), schema, Some(props()))?;
    if rows > 0 {
        w.write(&batch)?;
    }
    let (digest, bytes) = w.into_inner()?.finish();
    Ok((digest, bytes, rows))
}

/// One row of `aggregates.parquet`.
///
/// `group_key` is the ordered `k1=v1|k2=v2` rendering of IR §8.3 —
/// [`predictable_ir::run::Aggregation::group_key`] is the single implementation of it — and `t`
/// is `-1` unless the aggregation declared `over_t = "each"`.
#[derive(Debug, Clone, PartialEq)]
pub struct AggregateRow {
    pub aggregation: String,
    pub group_key: String,
    pub measure: String,
    pub t: i32,
    pub value: f64,
}

/// Write `run/aggregates.parquet`. Rows are written in the order given: aggregation declaration
/// order, then group key, then ascending `t` — the runner folds chunk partials in chunk index
/// order, never completion order (IR §8.3).
pub fn write_aggregates<W: Write + Send>(sink: W, rows: &[AggregateRow]) -> Result<ArtefactStats> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("aggregation", DataType::Utf8, false),
        Field::new("group_key", DataType::Utf8, false),
        Field::new("measure", DataType::Utf8, false),
        Field::new("t", DataType::Int32, false),
        Field::new("value", DataType::Float64, false),
    ]));
    let mut aggregation = StringBuilder::new();
    let mut group_key = StringBuilder::new();
    let mut measure = StringBuilder::new();
    let mut t = Int32Builder::new();
    let mut value = Float64Builder::new();
    for r in rows {
        aggregation.append_value(&r.aggregation);
        group_key.append_value(&r.group_key);
        measure.append_value(&r.measure);
        t.append_value(r.t);
        value.append_value(r.value);
    }
    let columns: Vec<ArrayRef> = vec![
        Arc::new(aggregation.finish()),
        Arc::new(group_key.finish()),
        Arc::new(measure.finish()),
        Arc::new(t.finish()),
        Arc::new(value.finish()),
    ];
    let (digest, bytes, n) = write_batch(sink, schema, columns)?;
    Ok(ArtefactStats {
        rows: n,
        digest,
        bytes,
    })
}

/// One modelpoint's outcome for a `scope = "per_mp"` solve (IR §8.4.4).
#[derive(Debug, Clone, PartialEq)]
pub struct SolveRow {
    pub mp_key: String,
    pub mp_row: u32,
    pub solved_value: f64,
    pub residual: f64,
    pub iterations: u32,
    pub converged: bool,
}

/// Write `run/solves/<name>.parquet` with the normative column list
/// `mp_key, mp_row, solved_value, residual, iterations, converged`.
pub fn write_solves<W: Write + Send>(sink: W, rows: &[SolveRow]) -> Result<ArtefactStats> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("mp_key", DataType::Utf8, false),
        Field::new("mp_row", DataType::UInt32, false),
        Field::new("solved_value", DataType::Float64, false),
        Field::new("residual", DataType::Float64, false),
        Field::new("iterations", DataType::UInt32, false),
        Field::new("converged", DataType::Boolean, false),
    ]));
    let mut mp_key = StringBuilder::new();
    let mut mp_row = UInt32Builder::new();
    let mut solved_value = Float64Builder::new();
    let mut residual = Float64Builder::new();
    let mut iterations = UInt32Builder::new();
    let mut converged = BooleanBuilder::new();
    for r in rows {
        mp_key.append_value(&r.mp_key);
        mp_row.append_value(r.mp_row);
        solved_value.append_value(r.solved_value);
        residual.append_value(r.residual);
        iterations.append_value(r.iterations);
        converged.append_value(r.converged);
    }
    let columns: Vec<ArrayRef> = vec![
        Arc::new(mp_key.finish()),
        Arc::new(mp_row.finish()),
        Arc::new(solved_value.finish()),
        Arc::new(residual.finish()),
        Arc::new(iterations.finish()),
        Arc::new(converged.finish()),
    ];
    let (digest, bytes, n) = write_batch(sink, schema, columns)?;
    Ok(ArtefactStats {
        rows: n,
        digest,
        bytes,
    })
}

/// A typed column of a table copy.
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnData {
    F64(Vec<f64>),
    I64(Vec<i64>),
    Bool(Vec<bool>),
    Str(Vec<String>),
}

impl ColumnData {
    fn len(&self) -> usize {
        match self {
            ColumnData::F64(v) => v.len(),
            ColumnData::I64(v) => v.len(),
            ColumnData::Bool(v) => v.len(),
            ColumnData::Str(v) => v.len(),
        }
    }

    fn field(&self, name: &str) -> Field {
        let dt = match self {
            ColumnData::F64(_) => DataType::Float64,
            ColumnData::I64(_) => DataType::Int64,
            ColumnData::Bool(_) => DataType::Boolean,
            ColumnData::Str(_) => DataType::Utf8,
        };
        Field::new(name, dt, false)
    }

    fn array(&self) -> ArrayRef {
        match self {
            ColumnData::F64(v) => {
                let mut b = Float64Builder::new();
                v.iter().for_each(|x| b.append_value(*x));
                Arc::new(b.finish())
            }
            ColumnData::I64(v) => {
                let mut b = Int64Builder::new();
                v.iter().for_each(|x| b.append_value(*x));
                Arc::new(b.finish())
            }
            ColumnData::Bool(v) => {
                let mut b = BooleanBuilder::new();
                v.iter().for_each(|x| b.append_value(*x));
                Arc::new(b.finish())
            }
            ColumnData::Str(v) => {
                let mut b = StringBuilder::new();
                v.iter().for_each(|x| b.append_value(x));
                Arc::new(b.finish())
            }
        }
    }
}

/// The content of one table, ready to be copied into the run directory (Q13).
///
/// The columns must be the ones decoded from **the same bytes the resolver hashed**, so the copy
/// is provably what was used rather than a re-read of a file that may since have changed.
#[derive(Debug, Clone, PartialEq)]
pub struct TableCopy {
    pub name: String,
    /// Key columns first, then value columns — the order the table declares them.
    pub columns: Vec<(String, ColumnData)>,
}

/// Write `run/tables/<name>.parquet`.
pub fn write_table_copy<W: Write + Send>(sink: W, table: &TableCopy) -> Result<ArtefactStats> {
    let rows = table.columns.first().map(|(_, c)| c.len()).unwrap_or(0);
    for (name, col) in &table.columns {
        if col.len() != rows {
            return Err(IoOutError::Io {
                path: format!("tables/{}.parquet", table.name),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("column `{name}` has {} rows, expected {rows}", col.len()),
                ),
            });
        }
    }
    let schema = Arc::new(Schema::new(
        table
            .columns
            .iter()
            .map(|(n, c)| c.field(n))
            .collect::<Vec<_>>(),
    ));
    let columns: Vec<ArrayRef> = table.columns.iter().map(|(_, c)| c.array()).collect();
    let (digest, bytes, n) = write_batch(sink, schema, columns)?;
    Ok(ArtefactStats {
        rows: n,
        digest,
        bytes,
    })
}

/// Rows, digest and size of a written artefact — the three things a manifest records.
#[derive(Debug, Clone, PartialEq)]
pub struct ArtefactStats {
    pub rows: u64,
    pub digest: String,
    pub bytes: u64,
}
