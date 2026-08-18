//! A Prophet `.rpt` as a first-class run directory (`04-verify.md` §4.3.7, §6.3 rule 5).
//!
//! The point of this module is that `predictable diff run_prophet/ run/` is the
//! *same code path* as diffing two predictable runs. So the import writes a real
//! `results.parquet` in the §2 long-format schema and a real `manifest.json` —
//! with `system = "prophet"` and everything the `.rpt` did not disclose written
//! as an explicit `null` rather than omitted or invented.

use std::collections::BTreeMap;
use std::path::Path;

use predictable_io::outbound::{
    manifest::{
        AssumptionInputs, ExecInfo, GitInfo, ModelInputs, ModelpointInputs, RunConfigInfo,
        SourceRef, TimelineInfo, Versions,
    },
    Cell, ComponentDescriptor, Environment, Execution, Inputs, IoOutError, Lineage, Manifest,
    ModelpointRows, Outcome, Provenance, ResultsChunk, ResultsRef, ResultsSchemaDoc, ResultsWriter,
    RunDir, WriterOptions,
};
use predictable_ir::run::{Emit, OnTrap, Retain, StoragePrecision};
use predictable_ir::{DType, Kind, Shape, Unit};

use crate::rpt::{RptLevel, RptResults};

/// Why a `.rpt` could not become a run directory.
#[derive(Debug)]
pub enum ImportError {
    /// The period base was never settled (`P0302`). An unrebased result set is
    /// not a run: every `t` in it would be a guess.
    UnresolvedPeriodBase {
        /// The `.rpt` path.
        file: String,
    },
    /// Writing the directory failed.
    Io(IoOutError),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::UnresolvedPeriodBase { file } => write!(
                f,
                "{file}: period base unresolved (P0302); pass --period-base before importing"
            ),
            ImportError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ImportError {}

impl From<IoOutError> for ImportError {
    fn from(e: IoOutError) -> ImportError {
        ImportError::Io(e)
    }
}

/// What only the caller knows: which module this Prophet run is being compared
/// against, and the bytes of the `.rpt` itself (for `inputs.modelpoints.source`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportOptions {
    /// The predictable module the run maps onto, e.g. `term_assurance`. Recorded
    /// in `inputs.model.module`; the digest stays empty because no model was run.
    pub module: Option<String>,
    /// The path recorded for the `.rpt`, if it differs from `results.file`.
    pub source_path: Option<String>,
    /// The command that produced this directory, for `provenance.invocation`.
    pub invocation: Option<String>,
}

/// The component descriptor list a `.rpt` import produces.
///
/// `timing` is `null`: a `.rpt` does not disclose whether a column is start-,
/// mid- or end-of-period, and inventing one would silently justify a timing
/// shift the diff should be reporting instead.
pub fn descriptors(results: &RptResults) -> Vec<ComponentDescriptor> {
    results
        .components
        .iter()
        .map(|c| ComponentDescriptor {
            id: c.id.clone(),
            name: c.source_name.clone(),
            kind: Kind::Output,
            dtype: DType::F64,
            shape: Shape::Series,
            unit: Unit::None,
            timing: None,
            stage: 1,
            output: true,
            display: None,
        })
        .collect()
}

/// Write `run_prophet/`: `results.parquet`, `results.schema.json` and
/// `manifest.json`.
///
/// Returns the built manifest. Fails only on IO and on a `.rpt` whose period base
/// was never settled — an unrebased result set is not a run.
pub fn write_run_dir(
    results: &RptResults,
    dir: impl AsRef<Path>,
    source_bytes: &[u8],
    options: &ImportOptions,
) -> Result<Manifest, ImportError> {
    if results.period_base.is_none() {
        return Err(ImportError::UnresolvedPeriodBase {
            file: results.file.clone(),
        });
    }

    let run = RunDir::create(dir)?;
    let schema = ResultsSchemaDoc::new("outputs", descriptors(results))?;
    let mut writer: ResultsWriter<std::fs::File> =
        run.results_writer(schema, WriterOptions::default())?;

    // One chunk, model points in `mp_row` order, because a `.rpt` is read whole.
    let mut by_mp: BTreeMap<u32, (String, Vec<Cell>)> = BTreeMap::new();
    for row in &results.rows {
        let entry = by_mp
            .entry(row.mp_row)
            .or_insert_with(|| (row.mp_key.clone(), Vec::new()));
        for (component, value) in results.components.iter().zip(&row.values) {
            if let Some(v) = value {
                entry.1.push(Cell::f64(component.id.clone(), row.t, *v));
            }
        }
    }
    let mut chunk = ResultsChunk::new(0);
    for (offset, (mp_row, (mp_key, cells))) in by_mp.into_iter().enumerate() {
        chunk.push(ModelpointRows {
            offset: offset as u32,
            mp_key,
            mp_row,
            cells,
        });
    }
    writer.write_chunk(&chunk)?;
    let summary = writer.finish()?;

    let source_digest = format!("sha256:{}", crate::sha256_hex(source_bytes));
    let source_path = options
        .source_path
        .clone()
        .unwrap_or_else(|| results.file.clone());

    let manifest = Manifest {
        format: String::new(), // stamped by `build()`
        kind: String::new(),
        manifest_digest: String::new(),
        run_id: results
            .run
            .clone()
            .unwrap_or_else(|| "prophet-import".to_string()),
        system: "prophet".to_string(),
        versions: Versions {
            ir_version: "pir/1".to_string(),
            engine_version: env!("CARGO_PKG_VERSION").to_string(),
            engine_git_sha: None,
            engine_build_profile: "prophet-import".to_string(),
            cli_version: None,
            dsl_version: None,
        },
        inputs: Inputs {
            model: ModelInputs {
                module: options.module.clone().unwrap_or_default(),
                product: results.product.clone(),
                digest: String::new(),
                files: Vec::new(),
            },
            assumptions: None::<AssumptionInputs>,
            modelpoints: ModelpointInputs {
                path: source_path.clone(),
                digest: source_digest.clone(),
                rows: results.mp_keys().len() as u64,
                key_field: results
                    .mp_key_column
                    .clone()
                    .unwrap_or_else(|| "<group>".to_string()),
                source: Some(SourceRef {
                    system: "prophet".to_string(),
                    file: source_path,
                    digest: source_digest,
                }),
            },
            tables: Vec::new(),
        },
        timeline: TimelineInfo {
            basis: results
                .time_units
                .as_deref()
                .map(|u| u.to_ascii_lowercase())
                .unwrap_or_else(|| "unknown".to_string()),
            periods: results
                .num_periods
                .unwrap_or_else(|| results.rows.iter().map(|r| r.t + 1).max().unwrap_or(0) as u32),
            origin: results
                .run_date
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            valuation_date: results.valuation_date.clone(),
            year_convention: None,
        },
        run_config: RunConfigInfo {
            path: String::new(),
            digest: String::new(),
            emit: Emit::Outputs,
            outputs: results.components.iter().map(|c| c.id.clone()).collect(),
            retain: Retain::Full,
            storage_precision: StoragePrecision::F64,
            on_trap: OnTrap::Abort,
            max_errors: 0,
            aggregations: Vec::new(),
            solves: Vec::new(),
            exec: ExecInfo {
                threads: 0,
                chunk_size: 0,
            },
            seed: None,
            flags: BTreeMap::from([(
                "aggregate_level".to_string(),
                results.level == RptLevel::Grouped,
            )]),
        },
        lineage: Lineage::base(),
        environment_hash: String::new(),
        environment: Environment {
            os: "unknown".to_string(),
            os_version: None,
            arch: "unknown".to_string(),
            cpu_features: Vec::new(),
            rustc: None,
            // The machine that ran Prophet is not this one, and the `.rpt` does
            // not say which it was. Stating "unknown" keeps the diff's
            // "same inputs, different machine" check honest.
            target_triple: "unknown".to_string(),
            float_settings: Default::default(),
            python: None,
            packages: BTreeMap::new(),
        },
        results: ResultsRef {
            path: "results.parquet".to_string(),
            digest: summary.digest.clone(),
            rows: summary.rows,
            components: summary.components,
            component_set_digest: summary.component_set_digest.clone(),
            aggregates: None,
        },
        execution: Execution {
            outcome: Outcome::Completed,
            started_at: results.run_date.clone().unwrap_or_default(),
            finished_at: results.run_date.clone().unwrap_or_default(),
            wall_ms: 0,
            modelpoints_projected: summary.modelpoints,
            modelpoints_trapped: 0,
            traps: Vec::new(),
            warnings: Vec::new(),
        },
        provenance: Provenance {
            invocation: options
                .invocation
                .clone()
                .unwrap_or_else(|| "predictable prophet rpt".to_string()),
            cwd_git: None::<GitInfo>,
            user: None,
        },
    };

    Ok(run.write_manifest(manifest)?)
}
