//! Substage levelling: the stage-2 → `init` back-channel (`03-engine.md` §5.3).
//!
//! `01-ir.md` §8.2 permits exactly one backward edge in the model: a stage-1
//! `init` may read a stage-2 value (`reserve.init = bel`, where `bel` is an
//! `npv` over the projection `reserve` itself takes part in). `expr` may not.
//! That single edge inverts the natural order — the `t` loop wants to run before
//! stage 2, and this one `init` wants stage 2 to have run first.
//!
//! The engine resolves it by **partitioning the work into levels** and running
//! the `t` loop once per level, over that level's slots only:
//!
//! ```text
//! level 0:  t loop over slots whose init reads no stage-2 value
//!           stage-2 slots whose series arguments are all level 0
//! level 1:  t loop over slots seeded from those stage-2 values
//!           stage-2 slots that reduce over level-1 series
//! ...
//! ```
//!
//! Two properties make this cheap rather than quadratic:
//!
//! * **The levels partition.** Every slot is in exactly one level, so the total
//!   op count is unchanged — levels cost extra *loop* traversals, not extra
//!   evaluation, and never extra memory: the arena is sized for the union.
//! * **Level 0 is the whole model in the ordinary case.** A model with no
//!   stage-2 `init` has one level, and [`Levels::is_flat`] lets the kernel take
//!   the original single-pass path with no per-range dispatch at all.
//!
//! ## Retention across levels
//!
//! A level-1 slot reading a level-0 series reads it *after* level 0's loop has
//! finished, so a `Ring` buffer would have wrapped away the history. Retention
//! is a layout decision (§4.3), so the engine repairs this where it is decided:
//! [`Levels::cross_level_reads`] names every series read from a higher level,
//! and [`crate::Layout::new`] gives those slots `Full` storage. The planner's
//! `Ring` choice is honoured everywhere it is still correct.
//!
//! ## The lint
//!
//! Each level is a full re-traversal of the projection. Three is generous — the
//! reference IFRS 17 model needs two — so a model with more than three levels
//! gets `W0110` ([`Levels::lints`]).

use std::collections::{BTreeMap, BTreeSet};

use predictable_diagnostics::Diagnostic;
use predictable_plan::{Plan, SlotId, Space};
use predictable_tape::{Op, Tape, TapeProgram};

/// The most levels a model may have before `W0110` fires (`03-engine.md` §5.3).
pub const MAX_QUIET_LEVELS: u32 = 3;

/// The substage assignment for one plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Levels {
    /// `SlotId::index()` → level. Slots that are not stage-1 series or stage-2
    /// values sit at level 0 and are never scheduled by the leveller.
    level: Vec<u32>,
    /// Stage-1 series slots per level, in plan order.
    stage1: Vec<Vec<SlotId>>,
    /// Stage-2 slots per level, in plan order.
    stage2: Vec<Vec<SlotId>>,
    /// Series slots read from a level above their own: they must be retained in
    /// full for the read to be meaningful.
    cross_level: BTreeSet<SlotId>,
    /// Stage-1 slots whose `init` reads a stage-2 value — the back-channel
    /// itself, reported by `explain()` and by the docs page.
    back_channel: BTreeMap<SlotId, BTreeSet<SlotId>>,
}

impl Levels {
    /// Level every slot of `plan`, reading the dependencies out of the lowered
    /// tapes rather than re-deriving them from expressions: the tape is what
    /// actually runs, so it is what the schedule must agree with.
    pub fn analyze(plan: &Plan, program: &TapeProgram) -> Levels {
        let n = plan.slot_refs.len();
        let stage1_set: BTreeSet<SlotId> = plan.stage1.iter().copied().collect();
        let stage2_set: BTreeSet<SlotId> = plan.stage2.iter().copied().collect();

        // `init` reads of stage-2 values, per stage-1 slot: the back-channel.
        let mut back_channel: BTreeMap<SlotId, BTreeSet<SlotId>> = BTreeMap::new();
        for (owner, reads) in seed_reads(&program.stage1.t0) {
            if !stage1_set.contains(&owner) {
                continue;
            }
            let s2: BTreeSet<SlotId> = reads
                .into_iter()
                .filter(|r| stage2_set.contains(r))
                .collect();
            if !s2.is_empty() {
                back_channel.insert(owner, s2);
            }
        }

        // What each stage-2 slot reads: its series arguments and any other
        // stage-2 slot it is arithmetic over.
        let mut s2_deps: BTreeMap<SlotId, BTreeSet<SlotId>> = BTreeMap::new();
        for (owner, reads) in range_reads(&program.stage2) {
            if !stage2_set.contains(&owner) {
                continue;
            }
            let deps = reads
                .into_iter()
                .filter(|r| stage1_set.contains(r) || stage2_set.contains(r))
                .collect();
            s2_deps
                .entry(owner)
                .or_default()
                .extend::<BTreeSet<_>>(deps);
        }

        // Fixed point. `G_init` is acyclic (`01-ir.md` §3.1, `E0202`), so this
        // converges; the bound is belt-and-braces, and on a cyclic input the
        // engine simply keeps the flat schedule and lets the checker speak.
        let mut level = vec![0u32; n];
        let bound = n + 1;
        let mut settled = false;
        for _ in 0..bound {
            let mut changed = false;
            for (&owner, deps) in &back_channel {
                let want = deps.iter().map(|d| level[d.index()] + 1).max().unwrap_or(0);
                if want > level[owner.index()] {
                    level[owner.index()] = want;
                    changed = true;
                }
            }
            for (&owner, deps) in &s2_deps {
                let want = deps.iter().map(|d| level[d.index()]).max().unwrap_or(0);
                if want > level[owner.index()] {
                    level[owner.index()] = want;
                    changed = true;
                }
            }
            if !changed {
                settled = true;
                break;
            }
        }
        if !settled {
            level = vec![0; n];
        }

        // A stage-1 slot must not run before anything it reads at lag 0 in the
        // same loop: propagate levels along instantaneous stage-1 edges too, so
        // a component seeded from stage 2 drags its consumers up with it.
        let stage1_deps: BTreeMap<SlotId, BTreeSet<SlotId>> = body_reads(program, &stage1_set);
        for _ in 0..bound {
            let mut changed = false;
            for (&owner, deps) in &stage1_deps {
                let want = deps.iter().map(|d| level[d.index()]).max().unwrap_or(0);
                if want > level[owner.index()] {
                    level[owner.index()] = want;
                    changed = true;
                }
            }
            for (&owner, deps) in &s2_deps {
                let want = deps.iter().map(|d| level[d.index()]).max().unwrap_or(0);
                if want > level[owner.index()] {
                    level[owner.index()] = want;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        let count = plan
            .stage1
            .iter()
            .chain(plan.stage2.iter())
            .map(|s| level[s.index()])
            .max()
            .unwrap_or(0)
            + 1;

        let mut stage1: Vec<Vec<SlotId>> = vec![Vec::new(); count as usize];
        let mut stage2: Vec<Vec<SlotId>> = vec![Vec::new(); count as usize];
        for &s in &plan.stage1 {
            stage1[level[s.index()] as usize].push(s);
        }
        for &s in &plan.stage2 {
            stage2[level[s.index()] as usize].push(s);
        }

        // Anything read from above its own level must survive the whole
        // projection, not just a ring window.
        let mut cross_level = BTreeSet::new();
        for (&owner, deps) in stage1_deps.iter().chain(s2_deps.iter()) {
            for d in deps {
                if plan.slot_refs[d.index()].space == Space::Series
                    && level[d.index()] < level[owner.index()]
                {
                    cross_level.insert(*d);
                }
            }
        }
        // The back-channel itself needs no retention repair: stage-2 values are
        // `PerMP`, one lane each, live for the whole chunk.

        Levels {
            level,
            stage1,
            stage2,
            cross_level,
            back_channel,
        }
    }

    /// A flat plan with no back-channel: one level, and the kernel keeps its
    /// single-pass hot path.
    pub fn flat(plan: &Plan) -> Levels {
        Levels {
            level: vec![0; plan.slot_refs.len()],
            stage1: vec![plan.stage1.clone()],
            stage2: vec![plan.stage2.clone()],
            cross_level: BTreeSet::new(),
            back_channel: BTreeMap::new(),
        }
    }

    /// How many substage levels this model needs. Always ≥ 1.
    pub fn count(&self) -> u32 {
        self.stage1.len() as u32
    }

    /// `true` when there is no back-channel at all.
    pub fn is_flat(&self) -> bool {
        self.count() == 1
    }

    /// The level a slot is scheduled in.
    pub fn level_of(&self, slot: SlotId) -> u32 {
        self.level[slot.index()]
    }

    /// Stage-1 slots to run in level `l`'s `t` loop.
    pub fn stage1(&self, l: u32) -> &[SlotId] {
        &self.stage1[l as usize]
    }

    /// Stage-2 slots to reduce after level `l`'s `t` loop.
    pub fn stage2(&self, l: u32) -> &[SlotId] {
        &self.stage2[l as usize]
    }

    /// Series slots read from a level above their own, which therefore need
    /// `Full` retention regardless of what the planner chose.
    pub fn cross_level_reads(&self) -> &BTreeSet<SlotId> {
        &self.cross_level
    }

    /// The back-channel itself: stage-1 slot → the stage-2 values its `init`
    /// reads.
    pub fn back_channel(&self) -> &BTreeMap<SlotId, BTreeSet<SlotId>> {
        &self.back_channel
    }

    /// `W0110` when the model needs more than three levels (§5.3).
    pub fn lints(&self, plan: &Plan) -> Vec<Diagnostic> {
        if self.count() <= MAX_QUIET_LEVELS {
            return Vec::new();
        }
        let deepest = self
            .stage1
            .last()
            .and_then(|l| l.first())
            .map(|s| plan.info(*s).qualified_id())
            .unwrap_or_else(|| "<none>".to_string());
        vec![Diagnostic::new(
            "W0110",
            format!(
                "this model needs {} stage-2 substage levels; each level is a full \
                 re-traversal of the projection",
                self.count()
            ),
        )
        .note(format!(
            "the deepest level is seeded from stage 2 through `{deepest}`"
        ))
        .note(
            "consider whether every component with a stage-2 `init` really needs a \
             prospective seed, or whether one of them can be a plain `init` constant",
        )]
    }

    /// A one-line-per-level rendering, for the CLI's plan report and the tests.
    pub fn text(&self, plan: &Plan) -> String {
        let name = |s: &SlotId| plan.info(*s).qualified_id();
        let mut out = String::new();
        for l in 0..self.count() {
            out.push_str(&format!(
                "level {l}: stage1 [{}] stage2 [{}]\n",
                self.stage1(l)
                    .iter()
                    .map(name)
                    .collect::<Vec<_>>()
                    .join(", "),
                self.stage2(l)
                    .iter()
                    .map(name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        out
    }
}

/// Slot reads per owning tape range, for the ranges that end in a `StoreSeed`
/// (the `init` sections of the `t = 0` tape).
fn seed_reads(tape: &Tape) -> Vec<(SlotId, BTreeSet<SlotId>)> {
    let mut out = Vec::new();
    for range in &tape.ranges {
        let ops = &tape.ops[range.start as usize..range.end as usize];
        if !ops.iter().any(|op| matches!(op, Op::StoreSeed(..))) {
            continue;
        }
        out.push((range.slot, reads_of(ops)));
    }
    out
}

/// Slot reads per owning tape range, for every range in a tape.
fn range_reads(tape: &Tape) -> Vec<(SlotId, BTreeSet<SlotId>)> {
    tape.ranges
        .iter()
        .map(|r| {
            (
                r.slot,
                reads_of(&tape.ops[r.start as usize..r.end as usize]),
            )
        })
        .collect()
}

/// Instantaneous stage-1 reads, unioned across every peeled body tape. `init`
/// sections are excluded: those are the back-channel, levelled separately.
fn body_reads(
    program: &TapeProgram,
    stage1: &BTreeSet<SlotId>,
) -> BTreeMap<SlotId, BTreeSet<SlotId>> {
    let mut out: BTreeMap<SlotId, BTreeSet<SlotId>> = BTreeMap::new();
    let tapes = std::iter::once(&program.stage1.t0)
        .chain(program.stage1.prefix.iter())
        .chain(std::iter::once(&program.stage1.body));
    for tape in tapes {
        for range in &tape.ranges {
            let ops = &tape.ops[range.start as usize..range.end as usize];
            if ops.iter().any(|op| matches!(op, Op::StoreSeed(..))) {
                continue;
            }
            let reads = reads_of(ops)
                .into_iter()
                .filter(|r| stage1.contains(r) && *r != range.slot)
                .collect::<BTreeSet<_>>();
            out.entry(range.slot).or_default().extend(reads);
        }
    }
    out
}

/// Every slot an op sequence reads. Writes (`Store*`) are deliberately not
/// counted: a slot storing to itself is not a dependency on itself.
fn reads_of(ops: &[Op]) -> BTreeSet<SlotId> {
    let mut out = BTreeSet::new();
    for op in ops {
        match op {
            Op::LoadScalar(_, s)
            | Op::LoadPerMp(_, s)
            | Op::LoadCur(_, s)
            | Op::LoadLag(_, s, _)
            | Op::LoadAt(_, s, _)
            | Op::LoadSeed(_, s)
            | Op::LoadHoistedCur(_, s)
            | Op::LoadHoistedLag(_, s, _)
            | Op::LoadHoistedAt(_, s, _)
            | Op::Cum(_, s, _) => {
                out.insert(*s);
            }
            Op::Reduce { series, pred, .. } => {
                out.insert(*series);
                out.extend(pred.iter().copied());
            }
            Op::Npv { value, disc, .. } => {
                out.insert(*value);
                out.insert(*disc);
            }
            _ => {}
        }
    }
    out
}
