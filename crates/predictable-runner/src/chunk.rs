//! The chunk pipeline's unit of work.
//!
//! [`predictable_io`] hands over dense, typed columns ([`ChunkColumns`]); the kernel wants
//! `f64` lanes ([`ChunkInput`]). The gap between them is the string dictionary: `str`, `enum`
//! and `date`-as-text values travel as dictionary codes in an ordinary `f64` lane
//! (`03-engine.md` §4.2), and only the [`Engine`] that will evaluate the chunk owns the
//! dictionary that assigns them.
//!
//! So a [`Chunk`] keeps text as text, and [`Chunk::bind`] interns it into whichever engine is
//! about to run it. That is what lets chunks be built once, on the loading thread, and executed
//! on any worker: codes never cross engines, and every comparison a tape performs is against a
//! literal interned by that same engine before any data was seen. Two workers may hand the same
//! string different codes; neither can observe the other's.

use std::collections::BTreeMap;

use predictable_engine::{ChunkInput, Engine};
use predictable_io::{ChunkColumns, ModelpointSource, MpColumn, MpSchema};

use crate::error::RunError;

/// One column of a loaded chunk, in the only two forms the kernel can consume.
#[derive(Debug, Clone, PartialEq)]
pub enum ChunkColumn {
    /// Already an `f64` lane: `f64`, `i64`, `bool` and `date`-as-epoch-days columns.
    Num(Vec<f64>),
    /// Text that must be interned by the executing engine: `str` and `enum` columns.
    Text(Vec<String>),
}

impl ChunkColumn {
    /// Rows held.
    pub fn len(&self) -> usize {
        match self {
            ChunkColumn::Num(v) => v.len(),
            ChunkColumn::Text(v) => v.len(),
        }
    }

    /// True when the column holds no rows.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A chunk of modelpoints, ready to be bound to an engine.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Chunk {
    /// Position in the file. Results are emitted in `(chunk_idx, offset)` order (`01-ir.md` §9.1).
    pub index: u32,
    /// File row index of lane 0.
    pub first_row: u64,
    /// Modelpoint keys, one per lane, in file order.
    pub keys: Vec<String>,
    /// Field name → lane values.
    pub columns: BTreeMap<String, ChunkColumn>,
}

impl Chunk {
    /// Lanes in this chunk.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// True when the chunk holds no modelpoints.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Bind the chunk to `engine`, interning text columns into *its* dictionary.
    pub fn bind(&self, engine: &mut Engine<'_>) -> ChunkInput {
        let mut columns = BTreeMap::new();
        for (name, column) in &self.columns {
            let lane = match column {
                ChunkColumn::Num(v) => v.clone(),
                ChunkColumn::Text(v) => v.iter().map(|s| engine.intern(s)).collect(),
            };
            columns.insert(name.clone(), lane);
        }
        ChunkInput {
            index: self.index,
            keys: self.keys.clone(),
            first_row: self.first_row,
            columns,
        }
    }
}

/// Convert one loaded [`ChunkColumns`] buffer into a [`Chunk`].
///
/// `key_field` names the `modelpoint_field` with `key = true` — the results join key
/// (`01-ir.md` §8.4.1). It must be present in the schema, or the run has no way to name a row.
pub fn chunk_from_columns(
    schema: &MpSchema,
    columns: &ChunkColumns,
    key_field: &str,
) -> Result<Chunk, RunError> {
    let rows = columns.rows();
    let mut out = BTreeMap::new();
    for field in schema.fields() {
        let column = columns
            .column(&field.name)
            .ok_or_else(|| RunError::MissingColumn(field.name.clone()))?;
        out.insert(field.name.clone(), lane_of(column, rows));
        // §2.11: the presence bit is a separate, opt-in `bool` lane beside the value lane.
        if let Some(presence) = columns.presence(&field.name) {
            out.insert(
                format!("{}__present", field.name),
                ChunkColumn::Num(presence.iter().map(|b| f64::from(u8::from(*b))).collect()),
            );
        }
    }

    let keys = match columns.column(key_field) {
        Some(MpColumn::Str(v)) => v.clone(),
        Some(MpColumn::I64(v)) => v.iter().map(|x| x.to_string()).collect(),
        Some(_) => return Err(RunError::BadKeyField(key_field.to_string())),
        None => return Err(RunError::MissingColumn(key_field.to_string())),
    };

    Ok(Chunk {
        index: columns.chunk_idx() as u32,
        first_row: columns.first_row(),
        keys,
        columns: out,
    })
}

fn lane_of(column: &MpColumn, rows: usize) -> ChunkColumn {
    match column {
        MpColumn::F64(v) => ChunkColumn::Num(v[..rows.min(v.len())].to_vec()),
        MpColumn::I64(v) => ChunkColumn::Num(v.iter().take(rows).map(|x| *x as f64).collect()),
        MpColumn::Bool(v) => ChunkColumn::Num(
            v.iter()
                .take(rows)
                .map(|x| f64::from(u8::from(*x)))
                .collect(),
        ),
        MpColumn::Date(v) => ChunkColumn::Num(v.iter().take(rows).map(|x| f64::from(*x)).collect()),
        MpColumn::Str(v) => ChunkColumn::Text(v[..rows.min(v.len())].to_vec()),
        MpColumn::Enum(v) => ChunkColumn::Num(v.iter().take(rows).map(|x| f64::from(*x)).collect()),
    }
}

/// Drain a [`ModelpointSource`] into chunks of exactly `chunk_size` rows (the last one short).
///
/// Chunk boundaries are a function of `chunk_size` and the row count only (`03-engine.md` §6),
/// so `run(C = 1) ≡ run(C = 1024)` is a property of the pipeline and not of the file's physical
/// batching. Chunks are materialised up front: a modelpoint file is columns of `f64`, the
/// projection is what costs memory, and having the whole work list in hand is what lets the
/// executor hand chunks out in index order and the solver replay them without re-reading.
///
/// **On column pruning.** `03-engine.md` §6 prunes a load to the fields some component reads.
/// The planner, though, allocates a `PerMP` input slot for *every* declared `modelpoint_field`
/// and the kernel binds all of them, so a chunk loaded through
/// [`predictable_io::MpSchema::pruned_for_module`] can leave a slot unbound and the run fails
/// with `MissingColumn`. Until the planner drops slots nothing reads, load with
/// [`predictable_io::MpSchema::from_module`]. Aggregation group keys are a second reason to:
/// a `group_by` field that no expression mentions still has to be in the chunk.
pub fn load_chunks(
    source: &mut dyn ModelpointSource,
    key_field: &str,
    chunk_size: usize,
) -> Result<Vec<Chunk>, RunError> {
    if chunk_size == 0 {
        return Err(RunError::BadChunkSize);
    }
    let schema = source.schema().clone();
    let mut buffer = ChunkColumns::for_schema(&schema);
    let mut chunks = Vec::new();
    loop {
        let rows = source.next_chunk(chunk_size, &mut buffer)?;
        if rows == 0 {
            break;
        }
        chunks.push(chunk_from_columns(&schema, &buffer, key_field)?);
    }
    Ok(chunks)
}

/// The chunk row index behind each surviving output lane.
///
/// A trapped modelpoint contributes no rows at all under `on_trap = "continue"` (`01-ir.md`
/// §9.3.1), so the surviving lanes of a [`predictable_engine::ChunkOutput`] are a *subsequence*
/// of the chunk's. Both the results writer (for `mp_row`) and the aggregation stage (for group
/// keys) need the mapping back, and it is computed once, here, rather than assumed twice.
pub fn surviving_rows(chunk: &Chunk, out: &predictable_engine::ChunkOutput) -> Vec<usize> {
    let mut rows = Vec::with_capacity(out.keys.len());
    let mut cursor = 0usize;
    for key in &out.keys {
        while cursor < chunk.keys.len() && &chunk.keys[cursor] != key {
            cursor += 1;
        }
        rows.push(cursor.min(chunk.keys.len().saturating_sub(1)));
        cursor += 1;
    }
    rows
}
