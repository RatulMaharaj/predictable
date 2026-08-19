//! The run directory: the on-disk assembly of everything above.
//!
//! ```text
//! run/
//!   manifest.json          pvf/1 manifest (§6)
//!   results.parquet        long format, (chunk_idx, offset) order (§2)
//!   results.schema.json    the component list, so no consumer parses .pir
//!   aggregates.parquet     portfolio aggregations (IR §8.3)      [optional]
//!   solves/<name>.parquet  per-modelpoint solve outcomes (IR §8.4.4)
//!   tables/<name>.parquet  table copies (Q13)
//! ```
//!
//! `manifest.json` is written **last**, because it records the digests of everything else.

use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::outbound::error::{IoOutError, Result};
use crate::outbound::manifest::Manifest;
use crate::outbound::schema::ResultsSchemaDoc;
use crate::outbound::sidecar::{
    write_aggregates, write_solves, write_table_copy, AggregateRow, ArtefactStats, SolveRow,
    TableCopy,
};
use crate::outbound::writer::{ResultsWriter, WriterOptions};

/// Relative paths inside a run directory. Manifest fields quote these verbatim.
pub mod paths {
    pub const MANIFEST: &str = "manifest.json";
    pub const RESULTS: &str = "results.parquet";
    pub const RESULTS_SCHEMA: &str = "results.schema.json";
    pub const AGGREGATES: &str = "aggregates.parquet";
    pub const SOLVES_DIR: &str = "solves";
    pub const TABLES_DIR: &str = "tables";
}

/// A run directory being written.
#[derive(Debug, Clone)]
pub struct RunDir {
    root: PathBuf,
}

impl RunDir {
    /// Create (or reuse) the directory and its `solves/` and `tables/` subdirectories.
    pub fn create(root: impl AsRef<Path>) -> Result<RunDir> {
        let root = root.as_ref().to_path_buf();
        for dir in [
            root.clone(),
            root.join(paths::SOLVES_DIR),
            root.join(paths::TABLES_DIR),
        ] {
            fs::create_dir_all(&dir).map_err(|e| IoOutError::io(dir.display().to_string(), e))?;
        }
        Ok(RunDir { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn create_file(&self, rel: &str) -> Result<File> {
        let path = self.path(rel);
        File::create(&path).map_err(|e| IoOutError::io(path.display().to_string(), e))
    }

    fn write_text(&self, rel: &str, text: &str) -> Result<()> {
        let path = self.path(rel);
        let mut f =
            File::create(&path).map_err(|e| IoOutError::io(path.display().to_string(), e))?;
        f.write_all(text.as_bytes())
            .map_err(|e| IoOutError::io(path.display().to_string(), e))
    }

    /// Write `results.schema.json` and open `results.parquet` for streaming.
    ///
    /// The schema document goes out first on purpose: if a run aborts mid-projection, the
    /// component set it *intended* to emit is still on disk and a consumer can say so.
    pub fn results_writer(
        &self,
        schema: ResultsSchemaDoc,
        options: WriterOptions,
    ) -> Result<ResultsWriter<File>> {
        self.write_text(paths::RESULTS_SCHEMA, &schema.to_canonical_json()?)?;
        let file = self.create_file(paths::RESULTS)?;
        ResultsWriter::new(file, schema, options)
    }

    /// Write `aggregates.parquet`.
    pub fn write_aggregates(&self, rows: &[AggregateRow]) -> Result<ArtefactStats> {
        write_aggregates(self.create_file(paths::AGGREGATES)?, rows)
    }

    /// Write `solves/<name>.parquet`; returns its run-relative path and stats.
    pub fn write_solve(&self, name: &str, rows: &[SolveRow]) -> Result<(String, ArtefactStats)> {
        let rel = format!("{}/{name}.parquet", paths::SOLVES_DIR);
        let stats = write_solves(self.create_file(&rel)?, rows)?;
        Ok((rel, stats))
    }

    /// Write `tables/<name>.parquet` (Q13); returns its run-relative path and stats.
    pub fn write_table_copy(&self, table: &TableCopy) -> Result<(String, ArtefactStats)> {
        let rel = format!("{}/{}.parquet", paths::TABLES_DIR, table.name);
        let stats = write_table_copy(self.create_file(&rel)?, table)?;
        Ok((rel, stats))
    }

    /// Build (stamping `environment_hash` and `manifest_digest`) and write `manifest.json`.
    /// Returns the built manifest so the caller can report the digests it just committed to.
    pub fn write_manifest(&self, manifest: Manifest) -> Result<Manifest> {
        let built = manifest.build()?;
        self.write_text(paths::MANIFEST, &built.to_canonical_json()?)?;
        Ok(built)
    }
}
