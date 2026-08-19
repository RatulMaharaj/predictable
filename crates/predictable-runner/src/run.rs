//! A whole run: solves, the final projection, the run directory and the manifest.
//!
//! This is where the pieces meet. In order (`01-ir.md` §8.4, §9.3.1, §9.4.1):
//!
//! 1. **Solves** run first, in declaration order, each one leaving its solved input in place, so
//!    the final projection is *the solved projection* and not an approximation of it (§8.4.4).
//! 2. **The final projection** is the one whose numbers reach `results.parquet`.
//! 3. **Trap policy** decides what is written: `abort` writes a manifest with
//!    `"outcome": "aborted"` and **no results**; `continue` writes results without the trapping
//!    modelpoints and `"outcome": "completed_with_traps"` (§9.3.1, Q7).
//! 4. **Table copies** travel with the result set (§9.4.1, Q13) unless suppressed, in which case
//!    `copy` is stamped `null` and consumers must say "table content unavailable in this run".
//! 5. **The manifest** carries the six reproducibility digests, `run_digest` (Q1), `lineage`
//!    (Q12) and every solve's outcome (Q8).

use std::path::Path;

use predictable_io::outbound::manifest::{ExecInfo, FileRef, SolveOutcome, TimelineInfo};
use predictable_io::outbound::{
    Cell, Inputs, Lineage, Manifest, ModelpointRows, Outcome, Provenance, ResultsChunk, ResultsRef,
    ResultsSchemaDoc, ResultsSummary, RunConfigInfo, RunDir, TableCopy, Versions, WriterOptions,
};
use predictable_ir::{DType, Timeline};

use crate::cancel::CancelFlag;
use crate::chunk::{surviving_rows, Chunk};
use crate::error::RunError;
use crate::executor::ChunkExecutor;
use crate::pipeline::{Projection, Runner};
use crate::solver::{self, SolveResult};

/// The provenance a run directory needs that the runner cannot know for itself.
#[derive(Debug, Clone)]
pub struct RunSpec {
    /// `manifest.run_id` — the run's name, typically its ISO start time plus a short hash.
    pub run_id: String,
    /// Path of the `[run]` file, recorded in `run_config.path`.
    pub run_path: String,
    /// Canonical text of the `[run]` file; `run_digest` is computed from it (§8.4.2).
    pub run_source: String,
    /// Model, assumption, modelpoint and table inputs, with their digests.
    pub inputs: Inputs,
    /// IR, engine, CLI and DSL versions.
    pub versions: Versions,
    /// Invocation, git state and user.
    pub provenance: Provenance,
    /// Q12: the sensitivity fan this run belongs to, if any.
    pub lineage: Lineage,
    /// Q13: the table content to copy into `run/tables/`, decoded from the bytes that were
    /// hashed. Empty means nothing to copy; `no_table_copy` means *deliberately* nothing.
    pub table_copies: Vec<TableCopy>,
    /// `--no-table-copy`: stamp `copy: null` and tell the consumer to say so.
    pub no_table_copy: bool,
    /// ISO-8601 start time. The only non-deterministic field in the manifest (§9.4).
    pub started_at: String,
    /// ISO-8601 finish time.
    pub finished_at: String,
    /// Wall time, milliseconds.
    pub wall_ms: u64,
}

/// What a finished run produced.
#[derive(Debug)]
pub struct RunReport {
    /// The built manifest, digests stamped.
    pub manifest: Manifest,
    /// `manifest.execution.outcome`.
    pub outcome: Outcome,
    /// `0` / `1` / `2` per `01-ir.md` §9.3.1.
    pub exit_code: i32,
    /// The final (solved) projection.
    pub projection: Projection,
    /// One entry per `[[solve]]`, in declaration order.
    pub solves: Vec<SolveResult>,
    /// `null` when the run aborted: no results file was written.
    pub results: Option<ResultsSummary>,
}

/// Run everything and write the run directory.
pub fn execute(
    runner: &mut Runner<'_>,
    mut chunks: Vec<Chunk>,
    executor: &dyn ChunkExecutor,
    cancel: &CancelFlag,
    spec: &RunSpec,
    out: &Path,
) -> Result<RunReport, RunError> {
    let solve_specs = runner.solves().to_vec();
    let mut solves = Vec::with_capacity(solve_specs.len());
    for solve in &solve_specs {
        let result = solver::solve(runner, solve, &mut chunks, executor, cancel)?;
        result.check(solve)?;
        solves.push(result);
    }

    let projection = runner.project(&chunks, executor, cancel)?;
    write_run(runner, &chunks, projection, solves, spec, out)
}

/// Write the run directory for an already-finished projection.
///
/// Split out from [`execute`] so a hosted or distributed runner (`03-engine.md` §10) can assemble
/// the same artefacts from chunks it did not project itself.
pub fn write_run(
    runner: &Runner<'_>,
    chunks: &[Chunk],
    projection: Projection,
    solves: Vec<SolveResult>,
    spec: &RunSpec,
    out: &Path,
) -> Result<RunReport, RunError> {
    let dir = RunDir::create(out)?;
    let mut inputs = spec.inputs.clone();

    // Q13: table content travels with the result set, or is explicitly stamped absent.
    if spec.no_table_copy {
        for table in &mut inputs.tables {
            table.copy = None;
            table.copy_digest = None;
        }
    } else {
        for copy in &spec.table_copies {
            let (path, stats) = dir.write_table_copy(copy)?;
            if let Some(entry) = inputs.tables.iter_mut().find(|t| t.name == copy.name) {
                *entry = entry.clone().with_copy(path, stats.digest, stats.rows);
            }
        }
    }

    let mut solve_blocks: Vec<SolveOutcome> = Vec::with_capacity(solves.len());
    for result in &solves {
        let mut block = result.outcome.clone();
        if !result.rows.is_empty() {
            let (path, stats) = dir.write_solve(&block.name, &result.rows)?;
            block.per_mp = Some(FileRef {
                path,
                digest: stats.digest,
            });
        }
        solve_blocks.push(block);
    }

    let mut results = None;
    if projection.outcome.has_results() {
        // Q8: the solved value is an ordinary `PerMP` component in the result set, so it needs
        // an ordinary descriptor too — a column the schema cannot name is a column nobody can
        // read.
        let mut components = runner.emitted().to_vec();
        for result in &solves {
            if result.solved.is_empty() {
                continue;
            }
            components.push(solve_descriptor(&result.outcome.name));
        }
        let schema = ResultsSchemaDoc::new(runner.emit_label(), components)?;
        let mut writer = dir.results_writer(schema, WriterOptions::default())?;
        for (chunk, out_chunk) in chunks.iter().zip(&projection.chunks) {
            let rows = surviving_rows(chunk, out_chunk);
            let mut batch = ResultsChunk::new(u64::from(out_chunk.index));
            for (lane, key) in out_chunk.keys.iter().enumerate() {
                let mut cells = Vec::new();
                for column in &out_chunk.columns {
                    let descriptor = writer.schema().require(&column.name)?;
                    let values = column.lane(lane);
                    if column.stride == 1 {
                        cells.push(cell(&descriptor.dtype, &column.name, -1, values[0])?);
                    } else {
                        for (t, v) in values.iter().enumerate() {
                            cells.push(cell(&descriptor.dtype, &column.name, t as i32, *v)?);
                        }
                    }
                }
                // Q8: the solved value is also an ordinary `PerMP` component, so downstream
                // joins do not have to special-case a solve.
                for result in &solves {
                    if let Some(v) = result.solved.get(key) {
                        cells.push(Cell::f64(result.outcome.name.clone(), -1, *v));
                    }
                }
                batch.push(ModelpointRows {
                    offset: lane as u32,
                    mp_key: key.clone(),
                    mp_row: (chunk.first_row + rows[lane] as u64) as u32,
                    cells,
                });
            }
            writer.write_chunk(&batch)?;
        }
        results = Some(writer.finish()?);
    }

    let aggregates = if projection.aggregates.is_empty() {
        None
    } else {
        let stats = dir.write_aggregates(&projection.aggregates)?;
        Some(predictable_io::outbound::manifest::AggregatesRef {
            path: predictable_io::outbound::run_dir::paths::AGGREGATES.to_string(),
            digest: stats.digest,
            rows: stats.rows,
        })
    };

    let results_ref = match &results {
        Some(summary) => ResultsRef {
            path: predictable_io::outbound::run_dir::paths::RESULTS.to_string(),
            digest: summary.digest.clone(),
            rows: summary.rows,
            components: summary.components,
            component_set_digest: summary.component_set_digest.clone(),
            aggregates,
        },
        None => ResultsRef {
            path: predictable_io::outbound::run_dir::paths::RESULTS.to_string(),
            digest: String::new(),
            rows: 0,
            components: runner.emitted().len(),
            // The set it *intended* to emit is still knowable, and still comparable.
            component_set_digest: predictable_fmt_component_set_digest(runner),
            aggregates,
        },
    };

    let execution = predictable_io::outbound::Execution {
        outcome: projection.outcome,
        started_at: spec.started_at.clone(),
        finished_at: spec.finished_at.clone(),
        wall_ms: spec.wall_ms,
        modelpoints_projected: projection.modelpoints_projected,
        modelpoints_trapped: projection.modelpoints_trapped,
        traps: projection.traps.iter().map(|t| t.envelope()).collect(),
        warnings: Vec::new(),
    };

    let run_config = RunConfigInfo {
        path: spec.run_path.clone(),
        digest: predictable_fmt::digest::run_digest(&spec.run_path, &spec.run_source).map_err(
            |e| RunError::BadSolve {
                solve: "[run]".to_string(),
                reason: format!("run file is not canonical: {e}"),
            },
        )?,
        emit: runner.run_file().run.emit,
        outputs: runner.emitted().iter().map(|c| c.id.clone()).collect(),
        retain: runner.run_file().run.retain,
        storage_precision: runner.run_file().run.storage_precision,
        on_trap: runner.run_file().run.on_trap,
        max_errors: runner.run_file().run.max_errors,
        aggregations: runner
            .run_file()
            .aggregations
            .iter()
            .map(|a| a.name.clone())
            .collect(),
        solves: solve_blocks,
        exec: ExecInfo {
            threads: runner.run_file().run.exec.threads.unwrap_or(1),
            chunk_size: runner.run_file().run.exec.chunk_size,
        },
        seed: None,
        flags: Default::default(),
    };

    let manifest = Manifest::draft(
        spec.run_id.clone(),
        spec.versions.clone(),
        inputs,
        timeline_info(runner.timeline()),
        run_config,
        results_ref,
        execution,
        spec.provenance.clone(),
    );
    let mut manifest = manifest;
    manifest.lineage = spec.lineage.clone();
    let manifest = dir.write_manifest(manifest)?;

    Ok(RunReport {
        outcome: projection.outcome,
        exit_code: projection.exit_code(),
        manifest,
        results,
        solves,
        projection,
    })
}

/// The descriptor of a solved value materialised as a component (§8.4.4).
fn solve_descriptor(name: &str) -> predictable_io::outbound::ComponentDescriptor {
    predictable_io::outbound::ComponentDescriptor {
        id: name.to_string(),
        name: name.to_string(),
        kind: predictable_ir::Kind::Output,
        dtype: DType::F64,
        shape: predictable_ir::Shape::PerMp,
        unit: predictable_ir::Unit::None,
        timing: None,
        stage: 1,
        output: true,
        display: None,
    }
}

fn predictable_fmt_component_set_digest(runner: &Runner<'_>) -> String {
    let ids: Vec<&str> = runner.emitted().iter().map(|c| c.id.as_str()).collect();
    predictable_fmt::digest::component_set_digest(&ids)
}

/// The lowercase wire spelling of an IR enum, taken from its own serde encoding rather than
/// re-spelled here — one spelling, one source.
fn wire<T: serde::Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn timeline_info(t: &Timeline) -> TimelineInfo {
    TimelineInfo {
        basis: wire(t.basis),
        periods: t.periods,
        origin: wire(t.origin),
        valuation_date: Some(t.valuation_date.clone()).filter(|s| !s.is_empty()),
        year_convention: Some(t.year_convention.clone()),
    }
}

/// The typed cell for one lane value. `f64` arithmetic is the only arithmetic (`01-ir.md` §2.4);
/// the dtype decides which `value*` column it lands in (`04-verify.md` §2).
fn cell(dtype: &DType, component: &str, t: i32, value: f64) -> Result<Cell, RunError> {
    Ok(match dtype {
        DType::F64 => Cell::f64(component, t, value),
        DType::I64 => Cell::i64(component, t, value as i64),
        DType::Bool => Cell::bool(component, t, value != 0.0),
        DType::Date => {
            let (y, m, d) = predictable_engine::dates::civil_from_days(value as i64);
            Cell::str(component, t, format!("{y:04}-{m:02}-{d:02}"))
        }
        // A `str`/`enum` lane holds a dictionary code private to the engine that produced it
        // (`03-engine.md` §4.2). Emitting the code would put a meaningless integer in a result
        // set, so the runner refuses rather than writing something a reader would misread.
        DType::Str | DType::Enum(_) => return Err(RunError::TextComponent(component.to_string())),
    })
}
