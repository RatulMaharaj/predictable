//! # `predictable-plan` — the planner
//!
//! The planner is the compiler. It runs once per `(Program, RunConfig)` and
//! produces a [`Plan`] that is `Send + Sync` and shared by every worker thread
//! (`03-engine.md` §3). It answers five questions, in this order:
//!
//! | Question | Answer | Spec |
//! |---|---|---|
//! | What can the runtime name? | a dense `u32` **slot** per value, partitioned by shape | §3.1 |
//! | In what order? | Kahn with a `(module_path, declaration_index)` min-heap, hashed into `order_digest` | §3.2, IR §3.2 |
//! | How much of each series is kept? | `Ring(max_lag + 1)` by default, `Full` when it is an output, reduced, or the target of an `At` | §4.3 |
//! | What can leave the per-modelpoint loop? | series whose whole dependency closure is modelpoint-independent | §3.4 |
//! | What can be computed now? | only rewrites that are exactly IEEE-preserving | §3.5 |
//!
//! Everything here is a *function of the checked model and the plan options*.
//! There is no IO, no threading, no clock and no allocator dependence: plan the
//! same model twice on two machines and the two [`Plan`]s compare equal, field
//! for field, digest for digest. That is the property the whole determinism
//! story (§7) rests on, so it is asserted directly in the tests rather than
//! inferred from results.
//!
//! ## Planning a model
//!
//! ```
//! use predictable_plan::{plan_sources, PlanOptions, Retention};
//! use predictable_check::Input;
//!
//! let source = r#"
//! format = "pir/1"
//! module = "term"
//!
//! [timeline]
//! basis = "annual"
//! periods = 10
//! origin = "policy"
//! valuation_date = 2026-06-30
//!
//! [[modelpoint_field]]
//! name = "sum_assured"
//! dtype = "f64"
//! unit = "money"
//! required = true
//!
//! [[assumption]]
//! name = "valuation_rate"
//! dtype = "f64"
//! shape = "Scalar"
//! unit = "rate(annual)"
//!
//! [[component]]
//! name = "discount"
//! kind = "Derived"
//! dtype = "f64"
//! shape = "Series"
//! unit = "factor"
//! timing = "start"
//! expr = "1.0 / (1.0 + valuation_rate) ^ t"
//!
//! [[component]]
//! name = "claims"
//! kind = "Output"
//! dtype = "f64"
//! shape = "Series"
//! unit = "money"
//! timing = "end"
//! expr = "sum_assured * 0.01"
//!
//! [[component]]
//! name = "pv_claims"
//! kind = "Output"
//! dtype = "f64"
//! shape = "PerMP"
//! unit = "money"
//! expr = "npv(claims, discount)"
//! "#;
//!
//! let plan = plan_sources(&[Input::new("term.pir", source)], &PlanOptions::default()).unwrap();
//!
//! // `discount` reads only the timeline and a scalar, so it leaves the loop.
//! assert!(plan.series_named("discount").unwrap().hoistable);
//! assert!(!plan.series_named("claims").unwrap().hoistable);
//!
//! // `claims` is an output *and* the argument of an `npv`: it is retained whole.
//! assert_eq!(plan.series_named("claims").unwrap().retention, Retention::Full);
//!
//! // Ordering is a total order over every slot, and it is hashed.
//! assert_eq!(plan.order_digest.len(), 64);
//! ```

#![deny(missing_debug_implementations)]
#![forbid(unsafe_code)]

pub mod deps;
pub mod digest;
pub mod fold;
pub mod order;
pub mod slots;

use std::collections::{BTreeMap, BTreeSet};

use predictable_diagnostics::Diagnostic;
use predictable_ir::{Expr, Kind, Module, Shape, Stage, Timing};
use serde::{Deserialize, Serialize};

use deps::{Read, Ref};
use order::{ModulePaths, Node, OrderError, OrderKey};
pub use slots::{
    FullReason, PerMpSlot, Retention, ScalarSlot, SeriesSlot, SlotId, SlotInfo, SlotRef, Space,
    TIMELINE_MODULE,
};

/// How hard the planner is allowed to work. `--O0` exists so a bit-level
/// difference can be bisected: it turns off *every* optional rewrite, leaving
/// only the passes that are semantically required (slots, order, retention).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptLevel {
    /// No folding, no hoisting. The plan still runs; it just does more work.
    O0,
    /// IEEE-preserving constant folding and loop-invariant hoisting.
    #[default]
    O1,
}

impl OptLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            OptLevel::O0 => "O0",
            OptLevel::O1 => "O1",
        }
    }
}

/// The plan-affecting part of a run configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlanOptions {
    pub opt: OptLevel,
    /// `--retain-all`: force every series slot to [`Retention::Full`], for
    /// whole-run drill-down in the UI (§4.3).
    pub retain_all: bool,
    /// The model digest from `predictable-fmt`, folded into [`Plan::digest`].
    /// Empty is legal and means "not pinned" — useful in tests, never in a run.
    pub program_digest: String,
    /// Override `T` when no `[timeline]` block is present (`01-ir.md` §8.4.2
    /// makes `[timeline].periods` the sole normative source when it exists).
    pub periods: Option<u32>,
}

impl PlanOptions {
    /// The canonical `key=value` rendering folded into the plan digest.
    fn run_config_text(&self) -> String {
        format!(
            "opt={}\nretain_all={}\nperiods={}\n",
            self.opt.as_str(),
            self.retain_all,
            self.periods.map(|p| p.to_string()).unwrap_or_default()
        )
    }
}

/// Why a model could not be planned.
#[derive(Debug)]
pub enum PlanError {
    /// The model does not check. The planner refuses unchecked input rather
    /// than half-planning it: diagnostics are the checker's product, and
    /// re-deriving them here would be a second, divergent opinion.
    NotChecked(Vec<Diagnostic>),
    /// `[timeline].periods` is absent and no override was supplied.
    NoTimeline,
    /// A cycle survived into the planner (see [`OrderError`]).
    Order(OrderError),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanError::NotChecked(d) => {
                write!(f, "model has {} error(s); planning refused", d.len())
            }
            PlanError::NoTimeline => f.write_str("no [timeline] block and no periods override"),
            PlanError::Order(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for PlanError {}

/// A compiled plan: everything the runtime needs that does not depend on data.
///
/// The four `Vec<SlotId>` fields are where the tapes of §3.3 will be lowered
/// (task T09); the planner's contract is the *order*, and nothing downstream may
/// reorder it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    /// `T` from `[timeline].periods`. The projection runs `t = 0..=periods`.
    pub periods: u32,
    pub scalars: Vec<ScalarSlot>,
    pub permp: Vec<PerMpSlot>,
    pub series: Vec<SeriesSlot>,
    /// `SlotId` → where it lives. Indexed by [`SlotId::index`].
    pub slot_refs: Vec<SlotRef>,
    /// `Scalar` and `PerMP` inputs and pure-`PerMP` derivations, computed before
    /// the `t` loop.
    pub prologue: Vec<SlotId>,
    /// Loop-invariant `Series`, computed once per run into a shared `(T+1)`
    /// array (§3.4).
    pub hoisted: Vec<SlotId>,
    /// The per-`t` body, in deterministic topological order.
    pub stage1: Vec<SlotId>,
    /// `Agg` reductions and the `PerMP` arithmetic over them, after the loop.
    pub stage2: Vec<SlotId>,
    /// `kind = "Output"` slots, in declaration order — the emission set.
    pub outputs: Vec<SlotId>,
    /// `sha256` of the slot ids in order (§3.2).
    pub order_digest: String,
    /// `sha256(program_digest ‖ run_config ‖ engine_semver_major)` (§3.1).
    pub digest: String,
    pub opt: OptLevel,
}

impl Plan {
    /// The whole evaluation order, prologue → hoisted → stage 1 → stage 2.
    pub fn order(&self) -> Vec<SlotId> {
        let mut out = Vec::new();
        out.extend_from_slice(&self.prologue);
        out.extend_from_slice(&self.hoisted);
        out.extend_from_slice(&self.stage1);
        out.extend_from_slice(&self.stage2);
        out
    }

    pub fn info(&self, id: SlotId) -> &SlotInfo {
        let r = self.slot_refs[id.index()];
        match r.space {
            Space::Scalar => &self.scalars[r.index as usize].info,
            Space::PerMp => &self.permp[r.index as usize].info,
            Space::Series => &self.series[r.index as usize].info,
        }
    }

    pub fn series_of(&self, id: SlotId) -> Option<&SeriesSlot> {
        let r = self.slot_refs[id.index()];
        (r.space == Space::Series).then(|| &self.series[r.index as usize])
    }

    pub fn series_named(&self, name: &str) -> Option<&SeriesSlot> {
        self.series.iter().find(|s| s.info.name == name)
    }

    pub fn permp_named(&self, name: &str) -> Option<&PerMpSlot> {
        self.permp.iter().find(|s| s.info.name == name)
    }

    pub fn scalar_named(&self, name: &str) -> Option<&ScalarSlot> {
        self.scalars.iter().find(|s| s.info.name == name)
    }

    /// The names in evaluation order — the readable form of [`Plan::order`],
    /// and what the docs page and the golden tests actually assert on.
    pub fn order_names(&self) -> Vec<String> {
        self.order()
            .into_iter()
            .map(|id| self.info(id).name.clone())
            .collect()
    }

    /// Bytes of `series_buf` per chunk of `chunk` modelpoints, given the
    /// retention decisions. This is the number `--retain-all` multiplies, and
    /// the CLI prints it before allocating (§4.3).
    pub fn series_bytes_per_chunk(&self, chunk: u32) -> u64 {
        self.series
            .iter()
            .filter(|s| !s.hoistable)
            .map(|s| s.retention.periods(self.periods) as u64 * chunk as u64 * 8)
            .sum()
    }
}

/// Plan a set of `.pir` sources: check them first, then plan.
pub fn plan_sources(
    inputs: &[predictable_check::Input],
    options: &PlanOptions,
) -> Result<Plan, PlanError> {
    let result = predictable_check::check(inputs);
    if !result.is_ok() {
        return Err(PlanError::NotChecked(
            result.errors().cloned().collect::<Vec<_>>(),
        ));
    }
    plan(&lower_modules(inputs), options)
}

/// Parse and lower the (already checked) inputs to IR modules.
///
/// Only module documents become modules: a product, run or assumption-set file
/// in the same set carries no declarations the planner allocates slots for.
pub fn lower_modules(inputs: &[predictable_check::Input]) -> Vec<Module> {
    let mut out = Vec::new();
    for input in inputs {
        let mut sources = predictable_syntax::SourceMap::new();
        let parsed =
            predictable_syntax::parse(&mut sources, input.name.clone(), input.text.clone());
        if parsed.document.kind() == predictable_syntax::ast::DocumentKind::Module {
            out.push(predictable_check::lower::module(&parsed.document));
        }
    }
    out
}

/// Plan a checked model.
///
/// `modules` must already have passed [`predictable_check::check`]; the planner
/// assumes resolution succeeded and `G₀` is acyclic, and returns
/// [`PlanError::Order`] rather than a diagnostic if that assumption is false.
pub fn plan(modules: &[Module], options: &PlanOptions) -> Result<Plan, PlanError> {
    let periods = modules
        .iter()
        .find_map(|m| m.timeline.as_ref().map(|t| t.periods))
        .or(options.periods)
        .ok_or(PlanError::NoTimeline)?;

    let mut b = Builder::new(options);
    b.declare(modules);
    b.analyse(modules);
    b.build(periods, options)
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

/// Fold an optional expression, or clone it verbatim under `--O0`.
fn maybe_fold(expr: Option<&Expr>, opt: OptLevel) -> Option<Expr> {
    match (expr, opt) {
        (Some(e), OptLevel::O1) => Some(fold::fold(e)),
        (Some(e), OptLevel::O0) => Some(e.clone()),
        (None, _) => None,
    }
}

/// One declaration, before it becomes a slot.
#[derive(Debug)]
struct Decl {
    info: SlotInfo,
    shape: Shape,
    timing: Timing,
    expr: Option<Expr>,
    init: Option<Expr>,
}

#[derive(Debug)]
struct Builder {
    decls: Vec<Decl>,
    by_name: BTreeMap<String, SlotId>,
    opt: OptLevel,
    /// Largest `k` of any `Lag(name, k)` in the whole model.
    max_lag: BTreeMap<String, u32>,
    /// Names read through an `At`, or reduced by a stage-2 `Agg`.
    at_targets: BTreeSet<String>,
    reduced: BTreeSet<String>,
}

impl Builder {
    fn new(options: &PlanOptions) -> Builder {
        Builder {
            decls: Vec::new(),
            by_name: BTreeMap::new(),
            opt: options.opt,
            max_lag: BTreeMap::new(),
            at_targets: BTreeSet::new(),
            reduced: BTreeSet::new(),
        }
    }

    fn push(&mut self, decl: Decl) {
        // Cross-module shadowing is `E0204`, reported by the checker; here the
        // first declaration wins so a shadow cannot allocate two slots for one
        // name and silently double the work.
        if self.by_name.contains_key(&decl.info.name) {
            return;
        }
        self.by_name.insert(decl.info.name.clone(), decl.info.id);
        self.decls.push(decl);
    }

    /// Allocate a slot for every declaration, in `(module, declaration)` order.
    fn declare(&mut self, modules: &[Module]) {
        let mut next = 0u32;

        for (i, (name, dtype, unit)) in slots::TIMELINE_FIELDS.iter().enumerate() {
            let slot = SlotId(next);
            next += 1;
            self.push(Decl {
                info: SlotInfo {
                    id: slot,
                    name: (*name).to_string(),
                    module_path: TIMELINE_MODULE.to_string(),
                    decl_index: i as u32,
                    kind: Kind::InputTimeline,
                    dtype: dtype.clone(),
                    unit: unit.clone(),
                    stage: Stage::One,
                },
                shape: Shape::Series,
                timing: Timing::Start,
                expr: None,
                init: None,
            });
        }

        for module in modules {
            let mut decl_index = 0u32;
            for f in &module.modelpoint_fields {
                let slot = SlotId(next);
                next += 1;
                let index = decl_index;
                decl_index += 1;
                self.push(Decl {
                    info: SlotInfo {
                        id: slot,
                        name: f.name.clone(),
                        module_path: module.module.clone(),
                        decl_index: index,
                        kind: Kind::InputModelpoint,
                        dtype: f.dtype.clone(),
                        unit: f.unit.clone(),
                        stage: Stage::One,
                    },
                    shape: Shape::PerMp,
                    timing: Timing::Start,
                    expr: None,
                    init: None,
                });
            }
            for a in &module.assumptions {
                let slot = SlotId(next);
                next += 1;
                let index = decl_index;
                decl_index += 1;
                self.push(Decl {
                    info: SlotInfo {
                        id: slot,
                        name: a.name.clone(),
                        module_path: module.module.clone(),
                        decl_index: index,
                        kind: Kind::InputAssumption,
                        dtype: a.dtype.clone(),
                        unit: a.unit.clone(),
                        stage: Stage::One,
                    },
                    shape: a.shape,
                    timing: Timing::Start,
                    expr: None,
                    init: None,
                });
            }
            for c in &module.components {
                let slot = SlotId(next);
                next += 1;
                let index = decl_index;
                decl_index += 1;
                let expr = maybe_fold(c.expr.as_ref(), self.opt);
                let init = maybe_fold(c.init.as_ref(), self.opt);
                self.push(Decl {
                    info: SlotInfo {
                        id: slot,
                        name: c.name.clone(),
                        module_path: module.module.clone(),
                        decl_index: index,
                        kind: c.kind,
                        dtype: c.dtype.clone(),
                        unit: c.unit.clone(),
                        stage: c.stage(),
                    },
                    shape: c.shape,
                    timing: c.timing.unwrap_or(Timing::End),
                    expr,
                    init,
                });
            }
        }
    }

    /// Walk every expression once and record the three facts retention needs.
    fn analyse(&mut self, _modules: &[Module]) {
        let mut max_lag: BTreeMap<String, u32> = BTreeMap::new();
        let mut at_targets = BTreeSet::new();
        let mut reduced = BTreeSet::new();
        for decl in &self.decls {
            for r in deps::refs_of(decl.expr.as_ref(), decl.init.as_ref()) {
                match r.read {
                    Read::Lag(k) => {
                        let e = max_lag.entry(r.name.clone()).or_insert(0);
                        *e = (*e).max(k);
                    }
                    // §3.3: the planner forces `Full` on any slot that is the
                    // target of an `At`. `x[0]` is the one case a ring could
                    // serve, and it is not worth a second code path in the
                    // kernel for the one period it saves.
                    Read::At(_) => {
                        at_targets.insert(r.name.clone());
                    }
                    Read::Cur | Read::Table => {}
                }
                if r.in_agg && r.read != Read::Table {
                    reduced.insert(r.name.clone());
                }
            }
        }
        self.max_lag = max_lag;
        self.at_targets = at_targets;
        self.reduced = reduced;
    }

    fn build(self, periods: u32, options: &PlanOptions) -> Result<Plan, PlanError> {
        let paths = ModulePaths::new(self.decls.iter().map(|d| d.info.module_path.as_str()));
        let key = |d: &Decl| OrderKey {
            module: paths.id(&d.info.module_path),
            decl_index: d.info.decl_index,
            slot: d.info.id.0,
        };

        // ---- hoisting (§3.4) ------------------------------------------------
        let hoistable = self.hoistable(options.opt);

        // ---- the four groups ------------------------------------------------
        // A group is a set of slots evaluated by one tape. Order within a group
        // is Kahn's; order between groups is fixed by the execution model.
        let mut prologue_nodes = Vec::new();
        let mut hoisted_nodes = Vec::new();
        let mut stage1_nodes = Vec::new();
        let mut stage2_nodes = Vec::new();
        let after_the_loop = self.after_the_loop();
        for decl in &self.decls {
            let target = match decl.shape {
                Shape::Series if hoistable.contains(&decl.info.id) => &mut hoisted_nodes,
                Shape::Series => &mut stage1_nodes,
                _ if after_the_loop.contains(&decl.info.id) => &mut stage2_nodes,
                _ => &mut prologue_nodes,
            };
            target.push((decl, key(decl)));
        }

        let order_group = |group: &[(&Decl, OrderKey)]| -> Result<Vec<SlotId>, PlanError> {
            let members: BTreeSet<SlotId> = group.iter().map(|(d, _)| d.info.id).collect();
            let nodes: Vec<Node> = group
                .iter()
                .map(|(decl, key)| Node {
                    slot: decl.info.id,
                    key: *key,
                    // Only `G₀` edges constrain order: lag 0, stage 1, and out
                    // of an `init` (an `init` is its own tape range, scheduled
                    // by the substage levelling of §5.3, not by this order).
                    preds: decl
                        .expr
                        .as_ref()
                        .map(deps::refs)
                        .unwrap_or_default()
                        .iter()
                        .filter(|r: &&Ref| r.is_instantaneous())
                        .filter_map(|r| self.by_name.get(&r.name).copied())
                        .filter(|id| members.contains(id))
                        .collect(),
                })
                .collect();
            order::kahn(&nodes).map_err(PlanError::Order)
        };

        let prologue = order_group(&prologue_nodes)?;
        let hoisted = order_group(&hoisted_nodes)?;
        let stage1 = order_group(&stage1_nodes)?;
        let stage2 = order_group(&stage2_nodes)?;

        // ---- materialise the slots -----------------------------------------
        let mut scalars = Vec::new();
        let mut permp = Vec::new();
        let mut series = Vec::new();
        let mut slot_refs = vec![
            SlotRef {
                space: Space::Scalar,
                index: 0
            };
            self.decls.len()
        ];
        let mut outputs = Vec::new();

        for decl in &self.decls {
            if decl.info.kind == Kind::Output {
                outputs.push(decl.info.id);
            }
            let space = match decl.shape {
                Shape::Scalar => Space::Scalar,
                Shape::PerMp => Space::PerMp,
                Shape::Series => Space::Series,
            };
            let index = match space {
                Space::Scalar => {
                    scalars.push(ScalarSlot {
                        info: decl.info.clone(),
                        expr: decl.expr.clone(),
                    });
                    scalars.len() - 1
                }
                Space::PerMp => {
                    permp.push(PerMpSlot {
                        info: decl.info.clone(),
                        expr: decl.expr.clone(),
                    });
                    permp.len() - 1
                }
                Space::Series => {
                    let max_lag = self.max_lag.get(&decl.info.name).copied().unwrap_or(0);
                    let (retention, full_reason) = self.retention(decl, max_lag, options);
                    series.push(SeriesSlot {
                        info: decl.info.clone(),
                        timing: decl.timing,
                        max_lag,
                        retention,
                        full_reason,
                        init: decl.init.clone(),
                        expr: decl.expr.clone(),
                        hoistable: hoistable.contains(&decl.info.id),
                    });
                    series.len() - 1
                }
            };
            slot_refs[decl.info.id.index()] = SlotRef {
                space,
                index: index as u32,
            };
        }

        let order_digest = digest::order_digest(&[
            ("prologue", &prologue),
            ("hoisted", &hoisted),
            ("stage1", &stage1),
            ("stage2", &stage2),
        ]);
        let plan_digest = digest::plan_digest(
            &options.program_digest,
            &options.run_config_text(),
            &order_digest,
        );

        Ok(Plan {
            periods,
            scalars,
            permp,
            series,
            slot_refs,
            prologue,
            hoisted,
            stage1,
            stage2,
            outputs,
            order_digest,
            digest: plan_digest,
            opt: options.opt,
        })
    }

    /// The non-`Series` slots that must be evaluated *after* the `t` loop.
    ///
    /// `01-ir.md` §2.2 computes a component's `stage` from its own expression —
    /// `2` iff it contains an `Agg` — and that field is normative, so the
    /// planner records it verbatim. But a `PerMP` component that merely *reads*
    /// a stage-2 value ("`margin = pv_claims * 1.05`") also cannot run before
    /// the loop, and §3.1 puts exactly that in the stage-2 tape: "`Agg`
    /// reductions **and PerMP arithmetic**". So the tape membership is the
    /// transitive closure of the IR's stage, computed here and kept separate
    /// from the recorded field rather than overwriting it.
    fn after_the_loop(&self) -> BTreeSet<SlotId> {
        let mut set: BTreeSet<SlotId> = self
            .decls
            .iter()
            .filter(|d| d.shape != Shape::Series && d.info.stage == Stage::Two)
            .map(|d| d.info.id)
            .collect();
        loop {
            let mut added = false;
            for decl in &self.decls {
                if decl.shape == Shape::Series || set.contains(&decl.info.id) {
                    continue;
                }
                let reads_stage2 = decl
                    .expr
                    .as_ref()
                    .map(deps::refs)
                    .unwrap_or_default()
                    .iter()
                    .any(|r| self.by_name.get(&r.name).is_some_and(|id| set.contains(id)));
                if reads_stage2 {
                    set.insert(decl.info.id);
                    added = true;
                }
            }
            if !added {
                return set;
            }
        }
    }

    /// §4.3. `Ring(max_lag + 1)` unless something needs the whole history.
    fn retention(
        &self,
        decl: &Decl,
        max_lag: u32,
        options: &PlanOptions,
    ) -> (Retention, Option<FullReason>) {
        let reason = if options.retain_all {
            Some(FullReason::RetainAll)
        } else if decl.info.kind == Kind::Output {
            Some(FullReason::Output)
        } else if self.reduced.contains(&decl.info.name) {
            Some(FullReason::Reduced)
        } else if self.at_targets.contains(&decl.info.name) {
            Some(FullReason::AtTarget)
        } else {
            None
        };
        match reason {
            Some(r) => (Retention::Full, Some(r)),
            // Ring lengths are rounded up to a power of two so indexing is
            // `t & (len - 1)`: no modulo, no branch (§4.3).
            None => (
                Retention::Ring {
                    len: (max_lag + 1).next_power_of_two(),
                },
                None,
            ),
        }
    }

    /// §3.4. A `Series` slot is hoistable iff its transitive dependency closure
    /// — at every lag, through `init`, and through lookup keys — contains no
    /// `PerMP` slot and no modelpoint field.
    ///
    /// Computed as a greatest fixed point: assume every series is hoistable,
    /// then repeatedly demote any series that reads something that is not. The
    /// closure is finite and monotone, so it converges in at most one pass per
    /// series; the loop below runs to a stable state rather than counting.
    fn hoistable(&self, opt: OptLevel) -> BTreeSet<SlotId> {
        let mut set: BTreeSet<SlotId> = BTreeSet::new();
        if opt == OptLevel::O0 {
            return set;
        }
        for decl in &self.decls {
            if decl.shape == Shape::Series {
                set.insert(decl.info.id);
            }
        }
        loop {
            let mut demoted = Vec::new();
            for decl in &self.decls {
                if decl.shape != Shape::Series || !set.contains(&decl.info.id) {
                    continue;
                }
                // A `Series` input other than the timeline has no formula to be
                // invariant in: only the timeline is modelpoint-independent by
                // construction.
                if decl.expr.is_none() && decl.info.kind != Kind::InputTimeline {
                    demoted.push(decl.info.id);
                    continue;
                }
                for r in deps::refs_of(decl.expr.as_ref(), decl.init.as_ref()) {
                    if r.read == Read::Table {
                        // The table itself is run-constant; its *keys* are
                        // ordinary refs and were walked with everything else.
                        continue;
                    }
                    let Some(&id) = self.by_name.get(&r.name) else {
                        continue;
                    };
                    let Some(dep) = self.decls.iter().find(|d| d.info.id == id) else {
                        continue;
                    };
                    let ok = match dep.shape {
                        Shape::Scalar => true,
                        Shape::PerMp => false,
                        Shape::Series => set.contains(&id),
                    };
                    if !ok {
                        demoted.push(decl.info.id);
                        break;
                    }
                }
            }
            if demoted.is_empty() {
                return set;
            }
            for id in demoted {
                set.remove(&id);
            }
        }
    }
}
