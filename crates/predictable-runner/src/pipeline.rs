//! The chunk pipeline (`03-engine.md` §5.1) and everything wrapped around it.
//!
//! ```text
//! prepare      Scalar slots + loop-invariant series, once per worker
//! for each chunk (executor's choice of thread):
//!     bind     Chunk -> ChunkInput (dictionary interning, §4.2)
//!     project  prologue -> t loop -> stage 2 -> emit
//! fold         aggregation partials, combined in CHUNK INDEX ORDER (§8.3)
//! ```
//!
//! Everything above the kernel that can change a number lives here: the emission set (`emit`,
//! §8.4.3), the trap policy (`on_trap`, §9.3.1), the solve outer loop (§8.4.4) and the
//! aggregation fold (§8.3). Nothing here computes an arithmetic result itself — that is the
//! kernel's job and only the kernel's.

use std::collections::BTreeMap;

use predictable_engine::{
    ChunkOutput, Engine, EngineError, RunConfig as EngineConfig, TrapPolicy, TrapReport,
};
use predictable_io::outbound::{AggregateRow, ComponentDescriptor, Outcome};
use predictable_ir::run::{Emit, OnTrap, RunFile};
use predictable_ir::{Component, Module, Timeline};
use predictable_plan::{Plan, Retention, SlotId, Space};
use predictable_tables::CompiledTable;
use predictable_tape::TapeProgram;

use crate::aggregate::AggregationStage;
use crate::cancel::CancelFlag;
use crate::chunk::Chunk;
use crate::error::RunError;
use crate::executor::{ChunkExecutor, ChunkResult, ChunkWorker};

/// Everything a run needs that is not the data itself.
#[derive(Debug)]
pub struct RunInputs<'a> {
    /// The checked, lowered modules — the source of the result set's component descriptors.
    pub modules: &'a [Module],
    /// The plan the tapes were lowered from.
    pub plan: &'a Plan,
    /// The lowered tapes.
    pub tapes: &'a TapeProgram,
    /// Tables, already resolved, digest-checked and compiled (`01-ir.md` §2.9.1).
    pub tables: Vec<CompiledTable>,
    /// `Input.Assumption` values by component name.
    pub assumptions: BTreeMap<String, f64>,
    /// The `[run]` file: `emit`, `on_trap`, `max_errors`, `[[solve]]`, `[[aggregation]]`.
    pub run: &'a RunFile,
}

/// What one pass of the pipeline produced.
#[derive(Debug)]
pub struct Projection {
    /// Completed chunks, in chunk index order.
    pub chunks: Vec<ChunkOutput>,
    /// `manifest.execution.outcome` (`01-ir.md` §9.3.1).
    pub outcome: Outcome,
    /// Retained trap reports, capped by `max_errors`; the counts below stay exact.
    pub traps: Vec<TrapReport>,
    /// Exact trap count, whether or not the report was retained.
    pub trap_total: u64,
    /// Modelpoints that produced rows.
    pub modelpoints_projected: u64,
    /// Modelpoints abandoned at a trap.
    pub modelpoints_trapped: u64,
    /// Rows of `aggregates.parquet`, folded in chunk index order.
    pub aggregates: Vec<AggregateRow>,
    stage: AggregationStage,
}

impl Projection {
    /// The value of an aggregation group — the residual source for a portfolio solve.
    pub fn aggregate(&self, name: &str, group_key: Option<&str>) -> Option<f64> {
        self.stage.value_of(name, group_key)
    }

    /// `0` completed, `1` completed with traps or cancelled, `2` aborted (`04-verify.md` §7).
    pub fn exit_code(&self) -> i32 {
        match self.outcome {
            Outcome::Completed => 0,
            Outcome::CompletedWithTraps | Outcome::Cancelled => 1,
            Outcome::Aborted => 2,
        }
    }

    /// The value of a `PerMP` component for every projected modelpoint, in file order.
    pub fn per_mp(&self, component: &str) -> Vec<(String, f64)> {
        let mut out = Vec::new();
        for chunk in &self.chunks {
            if let Some(column) = chunk.column(component) {
                for (lane, key) in chunk.keys.iter().enumerate() {
                    out.push((key.clone(), column.lane(lane)[0]));
                }
            }
        }
        out
    }
}

/// The runner: a plan, its tapes and its tables, bound to a run configuration.
#[derive(Debug)]
pub struct Runner<'a> {
    plan: Plan,
    tapes: &'a TapeProgram,
    timeline: Timeline,
    tables: Vec<CompiledTable>,
    assumptions: BTreeMap<String, f64>,
    config: EngineConfig,
    run: &'a RunFile,
    emitted: Vec<ComponentDescriptor>,
}

impl<'a> Runner<'a> {
    /// Bind the inputs, resolving the emission set before any data is touched.
    pub fn new(inputs: RunInputs<'a>) -> Result<Runner<'a>, RunError> {
        let timeline = inputs
            .modules
            .iter()
            .find_map(|m| m.timeline.clone())
            .unwrap_or(Timeline {
                basis: predictable_ir::Basis::Annual,
                periods: inputs.plan.periods,
                origin: predictable_ir::Origin::Policy,
                valuation_date: String::new(),
                year_convention: "act/365".to_string(),
            });

        let stage = AggregationStage::new(&inputs.run.aggregations);
        let mut wanted = emission_names(inputs.plan, inputs.modules, inputs.run)?;
        for name in stage.required_components() {
            if !wanted.contains(&name) {
                wanted.push(name);
            }
        }
        // A group key that is a modelpoint field is read from the chunk itself, in its canonical
        // text form (§8.3) — emitting it would put a per-engine dictionary code in the result
        // set. Only a *derived* key has to be emitted to be groupable.
        for name in stage.group_keys() {
            let is_input = slot_of(inputs.plan, &name)
                .map(|s| inputs.plan.info(s).kind.is_input())
                .unwrap_or(false);
            if !is_input && !wanted.contains(&name) {
                wanted.push(name);
            }
        }

        let mut plan = inputs.plan.clone();
        plan.outputs = resolve_slots(&plan, &wanted)?;
        let emitted = descriptors(inputs.modules, &plan);

        let config = EngineConfig {
            chunk: inputs.run.run.exec.chunk_size.max(1) as usize,
            on_trap: match inputs.run.run.on_trap {
                OnTrap::Abort => TrapPolicy::Abort,
                OnTrap::Continue => TrapPolicy::Continue,
            },
            max_errors: inputs.run.run.max_errors as usize,
        };

        Ok(Runner {
            plan,
            tapes: inputs.tapes,
            timeline,
            tables: inputs.tables,
            assumptions: inputs.assumptions,
            config,
            run: inputs.run,
            emitted,
        })
    }

    /// The component descriptors of the result set, sorted by id — what `results.schema.json`
    /// is built from.
    pub fn emitted(&self) -> &[ComponentDescriptor] {
        &self.emitted
    }

    /// The `[[solve]]` blocks of the run file, in declaration order.
    pub fn solves(&self) -> &[predictable_ir::run::Solve] {
        &self.run.solves
    }

    /// The run file this runner was bound to.
    pub fn run_file(&self) -> &RunFile {
        self.run
    }

    /// `outputs` | `all` | `list` — the `emit` mode, as `results.schema.json` spells it.
    pub fn emit_label(&self) -> &'static str {
        match self.run.run.emit {
            Emit::Outputs => "outputs",
            Emit::All => "all",
            Emit::List => "list",
        }
    }

    /// `T` from `[timeline].periods` — the sole normative source (`01-ir.md` §8.4.2).
    pub fn periods(&self) -> u32 {
        self.plan.periods
    }

    /// The timeline this run projects over.
    pub fn timeline(&self) -> &Timeline {
        &self.timeline
    }

    /// Assumption values, as perturbed by any portfolio solve.
    pub fn assumptions(&self) -> &BTreeMap<String, f64> {
        &self.assumptions
    }

    /// Override an assumption — the `scope = "portfolio"` solve's `vary` handle.
    pub fn set_assumption(&mut self, name: &str, value: f64) {
        self.assumptions.insert(name.to_string(), value);
    }

    /// Project every chunk and fold the aggregations.
    pub fn project(
        &self,
        chunks: &[Chunk],
        executor: &dyn ChunkExecutor,
        cancel: &CancelFlag,
    ) -> Result<Projection, RunError> {
        let make = || -> Box<dyn ChunkWorker + '_> {
            Box::new(EngineWorker::new(
                &self.plan,
                self.tapes,
                self.tables.clone(),
                &self.timeline,
                self.config.clone(),
                &self.assumptions,
            ))
        };
        let results = executor.map_chunks(chunks, &make, cancel);

        let mut stage = AggregationStage::new(&self.run.aggregations);
        let mut projection = Projection {
            chunks: Vec::new(),
            outcome: Outcome::Completed,
            traps: Vec::new(),
            trap_total: 0,
            modelpoints_projected: 0,
            modelpoints_trapped: 0,
            aggregates: Vec::new(),
            stage: AggregationStage::new(&self.run.aggregations),
        };

        for (chunk, result) in chunks.iter().zip(results) {
            match result {
                ChunkResult::Done(out) => {
                    projection.modelpoints_projected += out.keys.len() as u64;
                    projection.modelpoints_trapped += out.dropped.len() as u64;
                    projection.trap_total += out.traps.total() as u64;
                    for report in out.traps.reports() {
                        if projection.traps.len() < self.config.max_errors {
                            projection.traps.push(report.clone());
                        }
                    }
                    if !out.dropped.is_empty() || !out.traps.is_empty() {
                        projection.outcome = Outcome::CompletedWithTraps;
                    }
                    stage.absorb(chunk, &out)?;
                    projection.chunks.push(*out);
                }
                ChunkResult::Cancelled => {
                    // §9.3.1: a truncated result set that looks complete is the hazard; say so.
                    projection.outcome = Outcome::Cancelled;
                }
                ChunkResult::Failed(EngineError::Trapped(log)) => {
                    // `on_trap = "abort"`: no results are written, but the manifest still is.
                    projection.trap_total += log.total() as u64;
                    projection.modelpoints_trapped += log.modelpoints() as u64;
                    for report in log.reports() {
                        if projection.traps.len() < self.config.max_errors {
                            projection.traps.push(report.clone());
                        }
                    }
                    projection.outcome = Outcome::Aborted;
                    projection.chunks.clear();
                    projection.aggregates.clear();
                    return Ok(projection);
                }
                ChunkResult::Failed(e) => return Err(RunError::Engine(e)),
            }
        }

        projection.aggregates = stage.finish();
        projection.stage = stage;
        Ok(projection)
    }
}

/// One thread's engine and arena.
///
/// Construction can fail (a tape naming a table the run was not given), and it fails on a rayon
/// worker thread where a panic would surface as an abort rather than an error. So the worker
/// carries the failure and returns it from its first chunk.
struct EngineWorker<'a> {
    inner: Result<(Engine<'a>, predictable_engine::ChunkBuffers), EngineError>,
}

impl std::fmt::Debug for EngineWorker<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineWorker").finish_non_exhaustive()
    }
}

impl<'a> EngineWorker<'a> {
    fn new(
        plan: &'a Plan,
        tapes: &'a TapeProgram,
        tables: Vec<CompiledTable>,
        timeline: &Timeline,
        config: EngineConfig,
        assumptions: &BTreeMap<String, f64>,
    ) -> EngineWorker<'a> {
        let inner = Engine::new(plan, tapes, tables, timeline, config).and_then(|mut engine| {
            let mut bufs = engine.buffers();
            engine.prepare(assumptions, &mut bufs)?;
            Ok((engine, bufs))
        });
        EngineWorker { inner }
    }
}

impl ChunkWorker for EngineWorker<'_> {
    fn run(&mut self, chunk: &Chunk) -> Result<ChunkOutput, EngineError> {
        let (engine, bufs) = match &mut self.inner {
            Ok(pair) => pair,
            Err(e) => return Err(e.clone()),
        };
        let input = chunk.bind(engine);
        engine.run_chunk(bufs, &input)
    }
}

/// The qualified ids a run emits, per `emit` (`01-ir.md` §8.4.3).
fn emission_names(plan: &Plan, modules: &[Module], run: &RunFile) -> Result<Vec<String>, RunError> {
    let mut out: Vec<String> = plan
        .outputs
        .iter()
        .map(|s| plan.info(*s).qualified_id())
        .collect();
    match run.run.emit {
        Emit::Outputs => {}
        Emit::All => {
            // The migration mode: every `Derived` as well.
            for module in modules {
                for c in &module.components {
                    if matches!(c.kind, predictable_ir::Kind::Derived) {
                        let id = c.qualified_id(&module.module);
                        if !out.contains(&id) {
                            out.push(id);
                        }
                    }
                }
            }
        }
        Emit::List => {
            for name in &run.run.emit_list {
                let id = qualify(plan, name).ok_or_else(|| {
                    // E0106: `emit = "list"` named something that does not resolve.
                    RunError::UnknownComponent(name.clone())
                })?;
                if !out.contains(&id) {
                    out.push(id);
                }
            }
        }
    }
    Ok(out)
}

/// Resolve names to slots, refusing a `Series` the plan retains only as a ring.
fn resolve_slots(plan: &Plan, names: &[String]) -> Result<Vec<SlotId>, RunError> {
    let mut out = Vec::new();
    for name in names {
        let slot = slot_of(plan, name).ok_or_else(|| RunError::UnknownComponent(name.clone()))?;
        if plan.slot_refs[slot.index()].space == Space::Series {
            let series = plan
                .series_of(slot)
                .ok_or_else(|| RunError::UnknownComponent(name.clone()))?;
            if !series.hoistable && !matches!(series.retention, Retention::Full) {
                return Err(RunError::NotRetained(name.clone()));
            }
        }
        if !out.contains(&slot) {
            out.push(slot);
        }
    }
    Ok(out)
}

fn slot_of(plan: &Plan, name: &str) -> Option<SlotId> {
    let all = plan
        .scalars
        .iter()
        .map(|s| &s.info)
        .chain(plan.permp.iter().map(|s| &s.info))
        .chain(plan.series.iter().map(|s| &s.info));
    let mut fallback = None;
    for info in all {
        if info.qualified_id() == name {
            return Some(info.id);
        }
        if info.name == name && fallback.is_none() {
            fallback = Some(info.id);
        }
    }
    fallback
}

fn qualify(plan: &Plan, name: &str) -> Option<String> {
    slot_of(plan, name).map(|s| plan.info(s).qualified_id())
}

/// Descriptors for the emitted slots, taken from the IR rather than re-derived (`04-verify.md` §2).
///
/// A slot with no `[[component]]` behind it — a modelpoint field or an assumption pulled into the
/// emission set because an aggregation groups by it — is described from its plan slot instead.
/// The result set must describe every column it carries; a column the schema cannot name is a
/// column a consumer cannot read.
fn descriptors(modules: &[Module], plan: &Plan) -> Vec<ComponentDescriptor> {
    let mut out = Vec::new();
    for slot in &plan.outputs {
        let info = plan.info(*slot);
        let found = modules.iter().find_map(|m| {
            m.components
                .iter()
                .find(|c| c.name == info.name && m.module == info.module_path)
                .map(|c: &Component| ComponentDescriptor::from_ir(&m.module, c))
        });
        out.push(found.unwrap_or_else(|| ComponentDescriptor {
            id: info.qualified_id(),
            name: info.name.clone(),
            kind: info.kind,
            dtype: info.dtype.clone(),
            shape: match plan.slot_refs[slot.index()].space {
                Space::Scalar => predictable_ir::Shape::Scalar,
                Space::PerMp => predictable_ir::Shape::PerMp,
                Space::Series => predictable_ir::Shape::Series,
            },
            unit: info.unit.clone(),
            timing: plan.series_of(*slot).map(|s| s.timing),
            stage: u8::from(info.stage) as i8,
            output: info.kind.is_emitted(),
            display: None,
        }));
    }
    out
}
