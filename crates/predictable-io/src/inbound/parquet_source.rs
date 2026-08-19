//! [`ParquetSource`] — the preferred modelpoint source.
//!
//! The point of this file is **column pruning**. The [`MpSchema`] already knows the exact set of
//! fields the model reads, so the projection is pushed into the Parquet reader as a
//! `ProjectionMask` over the file's leaf columns: a model touching 7 of an MPF's 60 columns reads
//! 7 column chunks off disk, not 60 (`03-engine.md` §6).
//!
//! Row-group boundaries are deliberately ignored for chunking — the source yields exactly `C` rows
//! at a time, so chunk boundaries are a function of `C` and the row count only.

use std::fs::File;
use std::path::Path;

use parquet::arrow::arrow_reader::{ParquetRecordBatchReader, ParquetRecordBatchReaderBuilder};
use parquet::arrow::ProjectionMask;

use crate::error::{IoError, Lint};
use crate::inbound::arrow_source::ArrowSource;
use crate::inbound::{ChunkColumns, ModelpointSource};
use crate::schema::MpSchema;

/// The batch size handed to the Parquet reader. Independent of the projection chunk size `C`:
/// batches are re-cut to `C` rows by [`ArrowSource`].
const READ_BATCH_ROWS: usize = 8192;

/// Modelpoints from a Parquet file, with the schema projection pushed down.
#[derive(Debug)]
pub struct ParquetSource {
    inner: ArrowSource,
}

impl ParquetSource {
    /// Open `path` against `schema`, reading only the projected columns.
    ///
    /// Validation happens here, before the first row group is decoded: a missing required column
    /// is an error naming the file, the column and the components that reference it.
    pub fn open(schema: MpSchema, path: impl AsRef<Path>) -> Result<ParquetSource, IoError> {
        let path = path.as_ref();
        let file = path.display().to_string();
        let handle = File::open(path)?;
        Self::from_reader(schema, handle, file)
    }

    /// Open any seekable Parquet reader (a file, an in-memory `Bytes`) against `schema`.
    pub fn from_reader<R>(
        schema: MpSchema,
        reader: R,
        file: impl Into<String>,
    ) -> Result<ParquetSource, IoError>
    where
        R: parquet::file::reader::ChunkReader + 'static,
    {
        let file = file.into();
        let builder =
            ParquetRecordBatchReaderBuilder::try_new(reader).map_err(|e| IoError::Backend {
                file: file.clone(),
                message: e.to_string(),
            })?;

        // Column pruning: keep only the leaves whose root column is a field of the load plan.
        // Fields the file does not carry are simply absent from the mask; the schema validation in
        // `ArrowSource::new` decides whether that is an error (required) or a default (optional).
        let parquet_schema = builder.parquet_schema();
        let roots: Vec<usize> = (0..parquet_schema.root_schema().get_fields().len())
            .filter(|&i| {
                let name = parquet_schema.root_schema().get_fields()[i].name();
                schema.field(name).is_some()
            })
            .collect();
        let mask = ProjectionMask::roots(parquet_schema, roots);

        let reader: ParquetRecordBatchReader = builder
            .with_projection(mask)
            .with_batch_size(READ_BATCH_ROWS)
            .build()
            .map_err(|e| IoError::Backend {
                file: file.clone(),
                message: e.to_string(),
            })?;

        Ok(ParquetSource {
            inner: ArrowSource::new(schema, Box::new(reader), file)?,
        })
    }
}

impl ModelpointSource for ParquetSource {
    fn schema(&self) -> &MpSchema {
        self.inner.schema()
    }

    fn next_chunk(&mut self, n: usize, out: &mut ChunkColumns) -> Result<usize, IoError> {
        self.inner.next_chunk(n, out)
    }

    fn lints(&self) -> &[Lint] {
        self.inner.lints()
    }
}
