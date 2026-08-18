//! The run: prologue, hoisted series, the `t` loop, stage 2 (`03-engine.md` §5.1).
//!
//! ```text
//! prepare:   Scalar slots, then the loop-invariant series, once per run
//! per chunk: bind modelpoint columns -> PerMP lanes
//!            prologue      (PerMP derivations)
//!            for t in 0..=T: stage-1 tape for the peel region of t
//!            stage 2       (Reduce / Npv and the PerMP arithmetic over them)
//!            emit          Output slots, tagged with the chunk index
//! ```
//!
//! Nothing here threads, allocates in the loop, reads a clock or touches a file:
//! `predictable-engine` is a pure function of `(plan, tapes, tables, inputs)`.
//! Parallelism is a *partition of the modelpoint set* one layer up, in the
//! runner, and never a change to this file — which is what makes
//! `run(C = 1) ≡ run(C = 1024)` a property rather than a hope.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use predictable_ir::Timeline;
use predictable_plan::{Plan, SlotId, Space};
use predictable_tables::CompiledTable;
use predictable_tape::{Tape, TapeProgram};

use crate::buffers::ChunkBuffers;
use crate::dict::Dictionary;
use crate::exec::{self, Ctx, Mode, RunState, TableBinding, TrapHit};
use crate::layout::{Layout, Place};
use crate::levels::Levels;
use crate::terms::{self, AggTrace, DEFAULT_MAX_TERMS};
use crate::timeline::Clock;
use crate::traps::{TrapKind, TrapLog, TrapPolicy, TrapReport};

/// Run knobs the kernel honours. None of them may change a result bit (§7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunConfig {
    /// `C`: modelpoints per chunk. Always a power of two in production; any
    /// value is legal and none changes the answer.
    pub chunk: usize,
    /// What a trap does to the run (`01-ir.md` §9.3.1).
    pub on_trap: TrapPolicy,
    /// `--max-errors`: how many distinct trap reports are *retained*. The counts
    /// stay exact regardless.
    pub max_errors: usize,
}

impl Default for RunConfig {
    fn default() -> RunConfig {
        RunConfig {
            chunk: 1024,
            on_trap: TrapPolicy::Abort,
            // `01-ir.md` §9.3.1 fixes the default at 100; where it and
            // `03-engine.md` §5.5 (which says 20) disagree, the IR spec wins.
            max_errors: 100,
        }
    }
}

/// Why a run could not proceed.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineError {
    /// A `Lookup` names a table the run was not given.
    UnknownTable(String),
    /// A required modelpoint column was not in the chunk.
    MissingColumn { component: String },
    /// A required assumption had no value.
    MissingAssumption { component: String },
    /// More modelpoints than the arena holds.
    ChunkOverflow { got: usize, capacity: usize },
    /// A trap under `on_trap = "abort"`: the run stops and writes no results.
    Trapped(Box<TrapLog>),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::UnknownTable(t) => write!(f, "no compiled table named `{t}`"),
            EngineError::MissingColumn { component } => {
                write!(
                    f,
                    "modelpoint column `{component}` is missing from the chunk"
                )
            }
            EngineError::MissingAssumption { component } => {
                write!(f, "assumption `{component}` has no value")
            }
            EngineError::ChunkOverflow { got, capacity } => {
                write!(
                    f,
                    "chunk of {got} modelpoints exceeds the arena's {capacity}"
                )
            }
            EngineError::Trapped(log) => write!(
                f,
                "run aborted: {} trap(s) across {} modelpoint(s)",
                log.total(),
                log.modelpoints()
            ),
        }
    }
}

impl std::error::Error for EngineError {}

/// One chunk of modelpoints, columnar, exactly as `predictable-io` hands it over.
#[derive(Debug, Clone, Default)]
pub struct ChunkInput {
    /// Position of this chunk in the file — results are emitted in
    /// `(chunk_idx, offset)` order (`01-ir.md` §9.1).
    pub index: u32,
    /// Modelpoint keys, one per lane, for trap reports and results.
    pub keys: Vec<String>,
    /// Row index in the source file of lane 0.
    pub first_row: u64,
    /// Component name → one value per lane. Strings arrive dictionary-encoded.
    pub columns: BTreeMap<String, Vec<f64>>,
}

impl ChunkInput {
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

/// One output component's values for a chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputColumn {
    pub slot: SlotId,
    pub name: String,
    /// `Series` outputs: `(T+1)` values per surviving lane, lane-major.
    /// `PerMP` and `Scalar` outputs: one value per surviving lane.
    pub values: Vec<f64>,
    /// Values per lane — `T+1` for a series, `1` otherwise.
    pub stride: usize,
}

impl OutputColumn {
    /// The values for one surviving lane: `T+1` long for a series, `1` for a
    /// `PerMP` or `Scalar` output.
    pub fn lane(&self, lane: usize) -> &[f64] {
        &self.values[lane * self.stride..(lane + 1) * self.stride]
    }

    /// Lanes in this column.
    pub fn lanes(&self) -> usize {
        self.values.len().checked_div(self.stride).unwrap_or(0)
    }
}

/// What a chunk produced.
#[derive(Debug, Clone)]
pub struct ChunkOutput {
    pub index: u32,
    /// Keys of the modelpoints that survived, in file order.
    pub keys: Vec<String>,
    pub columns: Vec<OutputColumn>,
    /// Modelpoints abandoned at a trap under `on_trap = "continue"`.
    pub dropped: Vec<String>,
    pub traps: TrapLog,
}

impl ChunkOutput {
    /// An output component by qualified name (`term.claims`) or bare name.
    pub fn column(&self, name: &str) -> Option<&OutputColumn> {
        self.columns
            .iter()
            .find(|c| c.name == name || c.name.rsplit('.').next() == Some(name))
    }
}

/// The kernel, bound to a plan, its tapes and its tables.
#[derive(Debug)]
pub struct Engine<'a> {
    plan: &'a Plan,
    program: &'a TapeProgram,
    tables: Vec<CompiledTable>,
    bindings: Vec<TableBinding>,
    clock: Clock,
    layout: Layout,
    dict: Dictionary,
    config: RunConfig,
    state: RunState,
    levels: Levels,
}

impl<'a> Engine<'a> {
    /// Bind a plan, its tapes and its tables into a runnable kernel.
    ///
    /// Everything that can be decided without data is decided here: the storage
    /// layout, the table bindings, the dictionary codes for every string literal
    /// in the tapes. After this, running a chunk allocates nothing.
    pub fn new(
        plan: &'a Plan,
        program: &'a TapeProgram,
        tables: Vec<CompiledTable>,
        timeline: &Timeline,
        config: RunConfig,
    ) -> Result<Engine<'a>, EngineError> {
        let mut bindings = Vec::with_capacity(program.tables.len());
        for name in &program.tables {
            bindings.push(bind_table(&tables, name)?);
        }

        let mut n_regs = program.prologue.n_regs.max(program.stage2.n_regs);
        n_regs = n_regs
            .max(program.stage1.n_regs())
            .max(program.hoisted.n_regs());
        let n_accs = program
            .tapes()
            .iter()
            .map(|(_, t)| t.accumulators)
            .max()
            .unwrap_or(0);

        // Substage levels first: they can change retention (§5.3), and
        // retention is a layout decision.
        let levels = Levels::analyze(plan, program);
        let layout = Layout::with_full(
            plan,
            config.chunk,
            n_regs,
            n_accs,
            levels.cross_level_reads(),
        );
        let mut dict = Dictionary::new();
        // `false` and `true` take codes 0 and 1 so a `bool` lane — which is
        // already `0.0` / `1.0` — decodes to the text a compiled table's
        // dictionary key index compares against, with no special case in the
        // lookup path.
        dict.intern("false");
        dict.intern("true");
        // Literals are interned before any data, so codes are a function of the
        // model alone.
        for (_, tape) in program.tapes() {
            for s in &tape.const_s {
                dict.intern(s);
            }
        }
        let state = RunState::new(&layout);

        Ok(Engine {
            plan,
            program,
            tables,
            bindings,
            clock: Clock::new(timeline),
            layout,
            dict,
            config,
            state,
            levels,
        })
    }

    /// The storage decisions, for the CLI's footprint report and for tests.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// The plan this kernel is bound to.
    pub fn plan(&self) -> &'a Plan {
        self.plan
    }

    /// The compiled tables, in the order [`Engine::new`] received them.
    pub fn tables(&self) -> &[CompiledTable] {
        &self.tables
    }

    /// The resolved timeline.
    pub fn clock(&self) -> &Clock {
        &self.clock
    }

    /// The string dictionary: literal text ↔ lane code.
    pub fn dict(&self) -> &Dictionary {
        &self.dict
    }

    /// Run-level state — scalars and hoisted series. Read-only, and the
    /// `explain()` recorder's only route to a value the kernel computed once
    /// per run rather than once per chunk.
    pub fn state(&self) -> &RunState {
        &self.state
    }

    /// The read-only view every evaluation path shares.
    pub(crate) fn ctx(&self) -> Ctx<'_> {
        Ctx {
            plan: self.plan,
            layout: &self.layout,
            clock: &self.clock,
            compiled: &self.tables,
            bindings: &self.bindings,
            dict: &self.dict,
        }
    }

    /// Intern a string value so a modelpoint column can carry its code.
    pub fn intern(&mut self, value: &str) -> f64 {
        self.dict.intern(value)
    }

    /// Allocate this worker's arena (§4.4). One per thread, reused for every
    /// chunk.
    pub fn buffers(&self) -> ChunkBuffers {
        ChunkBuffers::new(&self.layout)
    }

    /// Scalars and loop-invariant series: once per run, before any chunk.
    pub fn prepare(
        &mut self,
        assumptions: &BTreeMap<String, f64>,
        bufs: &mut ChunkBuffers,
    ) -> Result<(), EngineError> {
        bufs.reset(1);
        for slot in self.plan.prologue.iter().copied() {
            let info = self.plan.info(slot);
            if !info.kind.is_input() || self.plan.slot_refs[slot.index()].space != Space::Scalar {
                continue;
            }
            let value = lookup_input(assumptions, &info.name).ok_or_else(|| {
                EngineError::MissingAssumption {
                    component: info.name.clone(),
                }
            })?;
            if let Place::Scalar { index } = self.layout.place(slot) {
                self.state.scalars[index] = value;
            }
        }

        // Scalar derivations only: the `PerMP` half of the prologue depends on
        // modelpoint data and is evaluated per chunk instead.
        let program = self.program;
        let scalar_ranges: Vec<_> = program
            .prologue
            .ranges
            .iter()
            .filter(|r| self.plan.slot_refs[r.slot.index()].space == Space::Scalar)
            .map(|r| r.start as usize..r.end as usize)
            .collect();
        for range in scalar_ranges {
            self.exec(bufs, &program.prologue, 0, 1, range);
        }

        // Loop-invariant series, once into the shared `(T+1)` array (§3.4).
        for t in 0..=self.layout.periods {
            let tape = program.hoisted.at(t);
            self.exec(bufs, tape, t, 1, 0..tape.len());
        }
        Ok(())
    }

    /// Project one chunk of modelpoints.
    pub fn run_chunk(
        &mut self,
        bufs: &mut ChunkBuffers,
        input: &ChunkInput,
    ) -> Result<ChunkOutput, EngineError> {
        let lanes = input.len();
        if lanes > bufs.capacity() {
            return Err(EngineError::ChunkOverflow {
                got: lanes,
                capacity: bufs.capacity(),
            });
        }
        bufs.reset(lanes);
        self.bind_modelpoints(bufs, input)?;

        let mut log = TrapLog::new(self.config.max_errors);

        let program = self.program;
        let prologue = &program.prologue;
        self.exec(bufs, prologue, 0, lanes, 0..prologue.len());
        self.drain_traps(bufs, input, prologue, 0, lanes, &mut log, None)?;

        if self.levels.is_flat() {
            // The ordinary model: one level, whole tapes, no per-range
            // dispatch. This is the path §5.3's levelling must not tax.
            for t in 0..=self.layout.periods {
                let tape = program.stage1.at(t);
                self.exec(bufs, tape, t, lanes, 0..tape.len());
                self.drain_traps(bufs, input, tape, t, lanes, &mut log, None)?;
            }

            let stage2 = &program.stage2;
            self.exec(bufs, stage2, 0, lanes, 0..stage2.len());
            self.drain_traps(bufs, input, stage2, 0, lanes, &mut log, None)?;
        } else {
            for level in 0..self.levels.count() {
                let stage1: BTreeSet<SlotId> = self.levels.stage1(level).iter().copied().collect();
                let stage2: BTreeSet<SlotId> = self.levels.stage2(level).iter().copied().collect();
                // `cum()` accumulators belong to the level's own `t` loop.
                bufs.reset_accs();
                for t in 0..=self.layout.periods {
                    let tape = program.stage1.at(t);
                    for range in subranges(tape, &stage1) {
                        self.exec(bufs, tape, t, lanes, range);
                    }
                    self.drain_traps(bufs, input, tape, t, lanes, &mut log, Some(&stage1))?;
                }
                let tape = &program.stage2;
                for range in subranges(tape, &stage2) {
                    self.exec(bufs, tape, 0, lanes, range);
                }
                self.drain_traps(bufs, input, tape, 0, lanes, &mut log, Some(&stage2))?;
            }
        }

        Ok(self.emit(bufs, input, log))
    }

    /// The substage schedule this model needs (`03-engine.md` §5.3).
    pub fn levels(&self) -> &Levels {
        &self.levels
    }

    /// `W0110` if the model needs more than three substage levels.
    pub fn level_lints(&self) -> Vec<predictable_diagnostics::Diagnostic> {
        self.levels.lints(self.plan)
    }

    /// Retain the per-`t` terms behind one `Agg` value, for `explain()`
    /// (`01-ir.md` §11.2, Q11).
    ///
    /// This is **replay-only**: it reads the buffers a completed
    /// [`Engine::run_chunk`] left behind and re-walks the reduction in the same
    /// order the kernel did, so the trace's contributions sum to the kernel's
    /// own value exactly. The hot loop is not involved and pays nothing —
    /// a run that never calls this never records a term.
    ///
    /// `component` is the qualified or bare name of a stage-2 component;
    /// `lane` is its position in the chunk that was just run. Returns `None`
    /// when the component is not an aggregate.
    pub fn agg_trace(
        &self,
        bufs: &ChunkBuffers,
        component: &str,
        lane: usize,
        max_terms: usize,
    ) -> Option<AggTrace> {
        let slot = self
            .plan
            .stage2
            .iter()
            .copied()
            .find(|s| matches_name(&self.plan.info(*s).qualified_id(), component))?;
        let tape = &self.program.stage2;
        let range = tape.ranges.iter().find(|r| r.slot == slot)?;
        let op = tape.ops[range.start as usize..range.end as usize]
            .iter()
            .find(|op| {
                matches!(
                    op,
                    predictable_tape::Op::Reduce { .. } | predictable_tape::Op::Npv { .. }
                )
            })?;

        let ctx = Ctx {
            plan: self.plan,
            layout: &self.layout,
            clock: &self.clock,
            compiled: &self.tables,
            bindings: &self.bindings,
            dict: &self.dict,
        };
        let t_max = self.layout.periods;

        let trace = match op {
            predictable_tape::Op::Reduce {
                agg, series, pred, ..
            } => {
                let mut get =
                    |t: u32| crate::reduce::value_at(&ctx, &self.state, bufs, *series, t, lane);
                let mut keep = |t: u32| match pred {
                    None => true,
                    Some(p) => crate::ops::truthy(crate::reduce::value_at(
                        &ctx,
                        &self.state,
                        bufs,
                        *p,
                        t,
                        lane,
                    )),
                };
                let (value, retained) =
                    terms::record_reduce(*agg, t_max, &mut get, &mut keep, pred.is_some());
                AggTrace {
                    node: "Agg",
                    op: terms::agg_name(*agg).to_string(),
                    reference: self.plan.info(*series).qualified_id(),
                    timing_used: None,
                    value,
                    term_count: retained.len(),
                    terms: retained,
                    terms_truncated: false,
                }
            }
            predictable_tape::Op::Npv {
                value: v,
                disc,
                timing,
                ..
            } => {
                let mut get =
                    |t: u32| crate::reduce::value_at(&ctx, &self.state, bufs, *v, t, lane);
                let mut disc_at = |t: u32| {
                    crate::reduce::npv_factor(&ctx, &self.state, bufs, *disc, *timing, t, lane)
                };
                let (value, retained) = terms::record_npv(t_max, &mut get, &mut disc_at);
                AggTrace {
                    node: "Agg",
                    op: "npv".to_string(),
                    reference: self.plan.info(*v).qualified_id(),
                    timing_used: Some(*timing),
                    value,
                    term_count: retained.len(),
                    terms: retained,
                    terms_truncated: false,
                }
            }
            _ => return None,
        };
        Some(trace.truncate(max_terms))
    }

    /// [`Engine::agg_trace`] at the `--trace-max-terms` default of 4096.
    pub fn agg_trace_default(
        &self,
        bufs: &ChunkBuffers,
        component: &str,
        lane: usize,
    ) -> Option<AggTrace> {
        self.agg_trace(bufs, component, lane, DEFAULT_MAX_TERMS)
    }

    // -- internals ---------------------------------------------------------

    fn exec(
        &mut self,
        bufs: &mut ChunkBuffers,
        tape: &Tape,
        t: u32,
        lanes: usize,
        range: std::ops::Range<usize>,
    ) {
        let ctx = Ctx {
            plan: self.plan,
            layout: &self.layout,
            clock: &self.clock,
            compiled: &self.tables,
            bindings: &self.bindings,
            dict: &self.dict,
        };
        exec::exec_tape(
            &ctx,
            &mut self.state,
            bufs,
            tape,
            t,
            &mut Mode::Lanes(lanes),
            range,
        );
    }

    /// Copy modelpoint columns into `PerMP` lanes.
    fn bind_modelpoints(
        &self,
        bufs: &mut ChunkBuffers,
        input: &ChunkInput,
    ) -> Result<(), EngineError> {
        for slot in self.plan.prologue.iter().copied() {
            let info = self.plan.info(slot);
            if !info.kind.is_input() || self.plan.slot_refs[slot.index()].space != Space::PerMp {
                continue;
            }
            let Place::PerMp { offset } = self.layout.place(slot) else {
                continue;
            };
            let column = lookup_column(&input.columns, &info.name).ok_or_else(|| {
                EngineError::MissingColumn {
                    component: info.name.clone(),
                }
            })?;
            for (lane, v) in column.iter().take(input.len()).enumerate() {
                bufs.permp[offset + lane] = *v;
            }
            bufs.mark_written(offset, input.len());
        }
        Ok(())
    }

    /// The once-per-period trap check of §5.5, and the scalar replay behind it.
    #[allow(clippy::too_many_arguments)]
    fn drain_traps(
        &mut self,
        bufs: &mut ChunkBuffers,
        input: &ChunkInput,
        tape: &Tape,
        t: u32,
        lanes: usize,
        log: &mut TrapLog,
        restrict: Option<&BTreeSet<SlotId>>,
    ) -> Result<(), EngineError> {
        if !bufs.traps.any() {
            return Ok(());
        }
        let flagged: Vec<usize> = bufs.traps.lanes().filter(|&c| c < lanes).collect();
        for lane in flagged {
            if bufs.dead.get(lane) {
                continue;
            }
            bufs.dead.set(lane);
            let mut hit: Option<TrapHit> = None;
            {
                let ctx = Ctx {
                    plan: self.plan,
                    layout: &self.layout,
                    clock: &self.clock,
                    compiled: &self.tables,
                    bindings: &self.bindings,
                    dict: &self.dict,
                };
                // Under levelling, the replay must see only the ops that have
                // actually run: a later level's slots have no values yet, and
                // attributing this lane's trap to one of them would be a lie.
                let whole: Range<usize> = 0..tape.len();
                let ranges = match restrict {
                    Some(set) => subranges(tape, set),
                    None => std::iter::once(whole).collect(),
                };
                for range in ranges {
                    exec::exec_tape(
                        &ctx,
                        &mut self.state,
                        bufs,
                        tape,
                        t,
                        &mut Mode::Replay {
                            lane,
                            hit: &mut hit,
                        },
                        range,
                    );
                    if hit.is_some() {
                        break;
                    }
                }
            }
            log.push(self.report(tape, input, lane, t, hit));
        }
        bufs.traps.clear();
        if self.config.on_trap == TrapPolicy::Abort {
            return Err(EngineError::Trapped(Box::new(log.clone())));
        }
        Ok(())
    }

    /// Turn a replayed trap into the `E0902` envelope of `01-ir.md` §9.3.1.
    fn report(
        &self,
        tape: &Tape,
        input: &ChunkInput,
        lane: usize,
        t: u32,
        hit: Option<TrapHit>,
    ) -> TrapReport {
        let hit = hit.unwrap_or(TrapHit {
            kind: TrapKind::NotFinite,
            site: None,
            op_index: 0,
            operands: Vec::new(),
        });
        // Provenance: a trapping op carries its own site; anything else is
        // attributed to the slot whose tape range contains it.
        let (slot, path) = match hit.site.and_then(|s| tape.sites.get(s.0 as usize)) {
            Some(site) => (Some(site.slot), site.path.clone()),
            None => {
                let owner = tape
                    .ranges
                    .iter()
                    .find(|r| (r.start as usize..r.end as usize).contains(&hit.op_index));
                (owner.map(|r| r.slot), "expr".to_string())
            }
        };
        let component = slot
            .map(|s| self.plan.info(s).qualified_id())
            .unwrap_or_else(|| "<unknown>".to_string());
        let operands = hit
            .operands
            .iter()
            .map(|(n, v)| (n.clone(), *v))
            .collect::<Vec<_>>();
        let detail = match hit.kind {
            TrapKind::DivByZero => "division by zero".to_string(),
            TrapKind::LogNonPositive => "ln of a non-positive value".to_string(),
            TrapKind::PowNan => "pow produced NaN".to_string(),
            TrapKind::LookupMiss => "table lookup missed".to_string(),
            TrapKind::IndexOutOfRange => "index out of range".to_string(),
            TrapKind::OverflowToInf => "overflow to infinity".to_string(),
            TrapKind::NotFinite => "value is not finite".to_string(),
        };
        TrapReport {
            kind: hit.kind,
            message: format!("{detail} evaluating `{component}` at t = {t}"),
            component,
            expr_path: path,
            mp_key: input.keys.get(lane).cloned().unwrap_or_default(),
            mp_row: input.first_row + lane as u64,
            t,
            operands,
        }
    }

    /// Gather the `Output` slots for the surviving lanes.
    fn emit(&self, bufs: &ChunkBuffers, input: &ChunkInput, log: TrapLog) -> ChunkOutput {
        let lanes = input.len();
        let live: Vec<usize> = (0..lanes).filter(|&c| !bufs.dead.get(c)).collect();
        let t_len = self.layout.periods as usize + 1;
        let mut columns = Vec::with_capacity(self.plan.outputs.len());

        for slot in self.plan.outputs.iter().copied() {
            let name = self.plan.info(slot).qualified_id();
            let (values, stride) = match self.layout.place(slot) {
                Place::Series { .. } => {
                    let mut v = Vec::with_capacity(live.len() * t_len);
                    for &lane in &live {
                        for t in 0..=self.layout.periods {
                            let base = self.layout.series_at(slot, t).unwrap_or(0);
                            v.push(bufs.series[base + lane]);
                        }
                    }
                    (v, t_len)
                }
                Place::Hoisted { offset, .. } => {
                    let mut v = Vec::with_capacity(live.len() * t_len);
                    for _ in &live {
                        v.extend_from_slice(&self.state.hoisted[offset..offset + t_len]);
                    }
                    (v, t_len)
                }
                Place::PerMp { offset } => {
                    (live.iter().map(|&c| bufs.permp[offset + c]).collect(), 1)
                }
                Place::Scalar { index } => (vec![self.state.scalars[index]; live.len()], 1),
            };
            columns.push(OutputColumn {
                slot,
                name,
                values,
                stride,
            });
        }

        ChunkOutput {
            index: input.index,
            keys: live
                .iter()
                .map(|&c| input.keys.get(c).cloned().unwrap_or_default())
                .collect(),
            dropped: (0..lanes)
                .filter(|&c| bufs.dead.get(c))
                .map(|c| input.keys.get(c).cloned().unwrap_or_default())
                .collect(),
            columns,
            traps: log,
        }
    }
}

/// The op ranges of a tape that compute `slots`, merged where they are
/// adjacent so a level's `t` loop is as few contiguous passes as possible.
///
/// A tape's ranges are already in execution order (`predictable-tape`'s
/// `TapeRange`), and running a subset of them is exactly what §5.3 means by
/// "the `t` loop once per level, over that level's slots only".
fn subranges(tape: &Tape, slots: &BTreeSet<SlotId>) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    for range in &tape.ranges {
        if !slots.contains(&range.slot) {
            continue;
        }
        let (start, end) = (range.start as usize, range.end as usize);
        match out.last_mut() {
            Some(last) if last.end == start => last.end = end,
            _ => out.push(start..end),
        }
    }
    out
}

/// `TableId` → compiled table and value column. A tape names either the table
/// (one value column) or `table.value`.
pub(crate) fn bind_table(
    tables: &[CompiledTable],
    name: &str,
) -> Result<TableBinding, EngineError> {
    if let Some(i) = tables.iter().position(|t| t.name() == name) {
        return Ok(TableBinding { table: i, value: 0 });
    }
    if let Some((table, value)) = name.rsplit_once('.') {
        if let Some(i) = tables.iter().position(|t| t.name() == table) {
            if let Some(v) = tables[i].value_position(value) {
                return Ok(TableBinding { table: i, value: v });
            }
        }
    }
    Err(EngineError::UnknownTable(name.to_string()))
}

/// Inputs are addressable by their qualified name or their bare one — the
/// caller should not have to know which module a component lives in.
fn lookup_input(map: &BTreeMap<String, f64>, name: &str) -> Option<f64> {
    map.get(name).copied().or_else(|| {
        name.rsplit_once('.')
            .and_then(|(_, bare)| map.get(bare).copied())
    })
}

fn lookup_column<'c>(map: &'c BTreeMap<String, Vec<f64>>, name: &str) -> Option<&'c Vec<f64>> {
    map.get(name)
        .or_else(|| name.rsplit_once('.').and_then(|(_, bare)| map.get(bare)))
}

/// Qualified or bare name match, the same convenience the rest of the kernel's
/// public surface offers.
fn matches_name(qualified: &str, name: &str) -> bool {
    qualified == name || qualified.rsplit('.').next() == Some(name)
}
