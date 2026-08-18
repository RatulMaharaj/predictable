//! # `predictable-tape` — lowering a plan to tapes
//!
//! The planner ([`predictable_plan`]) decides *what* is computed and *in what
//! order*. This crate decides *how*: it turns each slot's `Expr` into a flat,
//! branch-free, register-machine tape that the kernel executes with a single
//! forward pass over a `Vec` — no tree walk, no recursion, and therefore no
//! stack-depth limit, which matters on wasm (`03-engine.md` §3.3).
//!
//! Four things live here, and each is a decision the spec pins down:
//!
//! | Concern | Answer | Spec |
//! |---|---|---|
//! | What can the kernel execute? | the [`Op`] set — whole-lane, straight-line, no control flow | §3.3 |
//! | How many scratch registers? | linear scan over live intervals; typical models need < 8 | §3.3 |
//! | What about traps in an untaken `If` arm? | a **static** mask register per arm, from the syntactic `If` chain | `01-ir.md` §2.6 |
//! | What about lags below the origin? | three tapes, peeled at `max_lag_global`, resolved at lowering | §5.2 |
//!
//! ## The three tapes
//!
//! `t = 0` is the only period where `init` applies, and `t < max_lag_global` is
//! the only range where a lag can fall below the origin. Rather than test for
//! either inside the hot loop, the loop is peeled ([`LoopTapes`]):
//!
//! ```text
//! t0      : init seeds, then the t = 0 body   (every lag is pre-origin)
//! prefix  : one tape per t in 1 .. peel       (lags with k > t are pre-origin)
//! body    : t >= peel                         (every lag is in range)
//! ```
//!
//! With the usual `max_lag_global = 1` the prefix is empty and this is exactly
//! the spec's "three tapes"; the `Vec` generalises it to deeper lags without a
//! run-time branch, which a single prefix tape could not do.
//!
//! ## Worked example
//!
//! ```
//! use predictable_check::Input;
//! use predictable_plan::{plan_sources, PlanOptions};
//! use predictable_tape::lower_plan;
//!
//! let src = r#"
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
//! [[modelpoint_field]]
//! name = "q"
//! dtype = "f64"
//! unit = "prob"
//! required = true
//!
//! [[component]]
//! name = "survivors"
//! kind = "Derived"
//! dtype = "f64"
//! shape = "Series"
//! unit = "count"
//! timing = "start"
//! init = "1.0"
//! expr = "survivors[t-1] * (1 - q)"
//!
//! [[component]]
//! name = "claims"
//! kind = "Output"
//! dtype = "f64"
//! shape = "Series"
//! unit = "money"
//! timing = "end"
//! expr = "survivors * q * sum_assured"
//! "#;
//!
//! let plan = plan_sources(&[Input::new("term.pir", src)], &PlanOptions::default()).unwrap();
//! let prog = lower_plan(&plan).unwrap();
//!
//! // `survivors` has an `init`, so at t = 0 it is the seed, not the formula.
//! let t0 = prog.stage1.t0.text();
//! assert!(t0.contains("store.seed"));
//! assert!(t0.contains("load.seed"));
//!
//! // In the body the same lag is an ordinary in-range load.
//! assert!(prog.stage1.body.text().contains("load.lag"));
//!
//! // Registers are few, and the count is a property of the tape alone.
//! assert!(prog.stage1.body.n_regs <= 4);
//! ```

#![deny(missing_debug_implementations)]
#![forbid(unsafe_code)]

pub mod digest;
mod lower;
pub mod op;
pub mod regalloc;
pub mod tape;

use predictable_plan::{Plan, SlotId, Space};
use serde::{Deserialize, Serialize};

pub use lower::When;
pub use op::{AccId, CmpOp, ConstId, Fn1, Fn2, FnN, Op, Reg, RegRole, SiteId, TableId, TimeField};
pub use tape::{Site, Tape, TapeBuilder, TapeRange};

use lower::Lowering;
use op::Op as O;

/// Why a plan could not be lowered.
///
/// Every variant here is a case the checker should have rejected first, or a
/// construct the op set deliberately does not cover. Lowering never invents a
/// fallback: a tape that silently computes something else is worse than no
/// tape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LowerError {
    /// A name that is not a slot. The checker's resolve pass owns this
    /// (`E0201`); reaching it here means the plan was built from unchecked IR.
    UnknownName(String),
    /// An `Agg` whose operand is not a bare series reference. `sum(a * b)` needs
    /// a materialised temporary, which is a planner rewrite, not a lowering one.
    AggArgumentNotASeries {
        slot: SlotId,
        path: String,
    },
    /// `npv(x)` with no discount series.
    NpvNeedsDiscount {
        slot: SlotId,
    },
    /// An `Agg` or timing builtin applied to something that is not a `Series`.
    NotASeries {
        name: String,
        path: String,
    },
    /// A builtin outside `01-ir.md` §2.8, or one with no lane-level form.
    UnsupportedCall(String),
    BadArity {
        func: String,
        got: usize,
    },
    /// `shift(x, k)` with `k < 0` — a forward reference (`01-ir.md` §2.7).
    ForwardShift {
        slot: SlotId,
        k: i64,
    },
    NeedsSeriesArgument(String),
    NeedsLiteralArgument(String),
    /// A pre-origin `Lag` on a `date`/`str`/`enum` series with no `init`: those
    /// dtypes have no zero (`01-ir.md` §2.7).
    NoPreOriginValue {
        name: String,
        dtype: String,
    },
    /// More than 65,536 simultaneously live values in one tape.
    TooManyRegisters,
}

impl std::fmt::Display for LowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LowerError::UnknownName(n) => write!(f, "unresolved name `{n}` reached lowering"),
            LowerError::AggArgumentNotASeries { slot, path } => write!(
                f,
                "aggregate operand at `{path}` (slot {slot}) is not a series reference"
            ),
            LowerError::NpvNeedsDiscount { slot } => {
                write!(f, "npv on slot {slot} has no discount series")
            }
            LowerError::NotASeries { name, path } => {
                write!(f, "`{name}` at `{path}` is not a series")
            }
            LowerError::UnsupportedCall(n) => write!(f, "builtin `{n}` has no tape form"),
            LowerError::BadArity { func, got } => {
                write!(f, "builtin `{func}` called with {got} argument(s)")
            }
            LowerError::ForwardShift { slot, k } => {
                write!(f, "shift by {k} on slot {slot} is a forward reference")
            }
            LowerError::NeedsSeriesArgument(n) => {
                write!(f, "`{n}` requires a bare series reference")
            }
            LowerError::NeedsLiteralArgument(n) => {
                write!(f, "`{n}` requires a literal integer argument")
            }
            LowerError::NoPreOriginValue { name, dtype } => write!(
                f,
                "`{name}` is `{dtype}` and has no `init`: there is no pre-origin value"
            ),
            LowerError::TooManyRegisters => f.write_str("tape needs more than 65536 registers"),
        }
    }
}

impl std::error::Error for LowerError {}

/// The peeled tapes of one `t` loop (`03-engine.md` §5.2).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LoopTapes {
    /// `init` seeds followed by the `t = 0` body.
    pub t0: Tape,
    /// One tape per `t` in `1 .. peel`, in ascending `t`.
    pub prefix: Vec<Tape>,
    /// `t >= peel`.
    pub body: Tape,
    /// `max_lag_global`: the first period at which every lag is in range.
    pub peel: u32,
}

impl LoopTapes {
    /// The tape that runs at period `t`.
    pub fn at(&self, t: u32) -> &Tape {
        if t == 0 {
            &self.t0
        } else if (t as usize) < self.prefix.len() + 1 {
            &self.prefix[t as usize - 1]
        } else {
            &self.body
        }
    }

    /// Widest register frame across the peel — what the kernel allocates.
    pub fn n_regs(&self) -> u16 {
        std::iter::once(self.t0.n_regs)
            .chain(self.prefix.iter().map(|t| t.n_regs))
            .chain(std::iter::once(self.body.n_regs))
            .max()
            .unwrap_or(0)
    }
}

/// Every tape a run needs, in execution order (`03-engine.md` §5.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TapeProgram {
    pub periods: u32,
    /// `max_lag_global` — the peel point shared by both `t` loops.
    pub peel: u32,
    /// `Scalar` / `PerMP` inputs and derivations, once per run and per chunk.
    pub prologue: Tape,
    /// Loop-invariant `Series`, once per run into the shared `(T+1)` array.
    pub hoisted: LoopTapes,
    /// The per-modelpoint projection.
    pub stage1: LoopTapes,
    /// `Reduce` / `Npv` and the `PerMP` arithmetic over them.
    pub stage2: Tape,
    /// `TableId` → table name, in first-use order.
    pub tables: Vec<String>,
    /// `sha256` over the canonical text of every tape (see [`digest`]).
    pub digest: String,
}

impl TapeProgram {
    /// Total ops across every tape — the number the CLI prints and the
    /// benchmarks track.
    pub fn op_count(&self) -> usize {
        self.prologue.len()
            + self.stage2.len()
            + [&self.hoisted, &self.stage1]
                .iter()
                .map(|l| {
                    l.t0.len() + l.body.len() + l.prefix.iter().map(|t| t.len()).sum::<usize>()
                })
                .sum::<usize>()
    }

    /// Every tape with its label, in execution order. The one place the
    /// program's structure is enumerated, so the printer, the digest and any
    /// consumer walk the same list.
    pub fn tapes(&self) -> Vec<(String, &Tape)> {
        let mut out: Vec<(String, &Tape)> = vec![("prologue".to_string(), &self.prologue)];
        for (label, l) in [("hoisted", &self.hoisted), ("stage1", &self.stage1)] {
            out.push((format!("{label}.t0"), &l.t0));
            for (i, t) in l.prefix.iter().enumerate() {
                out.push((format!("{label}.prefix[t={}]", i + 1), t));
            }
            out.push((format!("{label}.body"), &l.body));
        }
        out.push(("stage2".to_string(), &self.stage2));
        out
    }

    /// The canonical text of the whole program: what the golden tests diff.
    ///
    /// Constant pools are printed with the tape that owns them — an op says
    /// `constf #0`, and without the pool that line would be the same whether the
    /// model said `1.0` or `2.0`.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for (name, tape) in self.tapes() {
            out.push_str(&format!("# {name} (n_regs={})\n", tape.n_regs));
            for (i, x) in tape.const_f.iter().enumerate() {
                out.push_str(&format!("    constf #{i} = {x:?}\n"));
            }
            for (i, x) in tape.const_i.iter().enumerate() {
                out.push_str(&format!("    consti #{i} = {x}\n"));
            }
            for (i, x) in tape.const_s.iter().enumerate() {
                out.push_str(&format!("    consts #{i} = {x:?}\n"));
            }
            out.push_str(&tape.text());
        }
        out
    }
}

/// Lower a [`Plan`] to its tapes.
pub fn lower_plan(plan: &Plan) -> Result<TapeProgram, LowerError> {
    let mut lo = Lowering::new(plan);

    // The peel point: the deepest lag anywhere in the model, clamped to the
    // projection length. Beyond it, no `LoadLag` can be pre-origin.
    let peel = plan
        .series
        .iter()
        .map(|s| s.max_lag)
        .max()
        .unwrap_or(0)
        .min(plan.periods);

    let prologue = straight_line(&mut lo, &plan.prologue.clone())?;
    let stage2 = straight_line(&mut lo, &plan.stage2.clone())?;
    let hoisted = loop_tapes(&mut lo, &plan.hoisted.clone(), peel, true)?;
    let stage1 = loop_tapes(&mut lo, &plan.stage1.clone(), peel, false)?;

    let mut prog = TapeProgram {
        periods: plan.periods,
        peel,
        prologue,
        hoisted,
        stage1,
        stage2,
        tables: lo.tables.clone(),
        digest: String::new(),
    };
    prog.digest = digest::tape_digest(&prog);
    Ok(prog)
}

/// A tape with no `t` loop: prologue and stage 2.
fn straight_line(lo: &mut Lowering<'_>, slots: &[SlotId]) -> Result<Tape, LowerError> {
    let mut b = TapeBuilder::new();
    for &id in slots {
        let expr = match lo.plan.slot_refs[id.index()].space {
            Space::Scalar => lo.plan.scalars[lo.plan.slot_refs[id.index()].index as usize]
                .expr
                .clone(),
            Space::PerMp => lo.plan.permp[lo.plan.slot_refs[id.index()].index as usize]
                .expr
                .clone(),
            Space::Series => None,
        };
        let Some(expr) = expr else { continue };
        let start = b.mark();
        let r = lo.lower_component(&mut b, id, &expr, "expr", When::Body)?;
        b.emit(match lo.plan.slot_refs[id.index()].space {
            Space::Scalar => O::StoreScalar(id, r),
            _ => O::StorePerMp(id, r),
        });
        b.range(id, start);
    }
    Ok(b.finish())
}

/// The peeled `t` loop for a group of `Series` slots.
fn loop_tapes(
    lo: &mut Lowering<'_>,
    slots: &[SlotId],
    peel: u32,
    hoisted: bool,
) -> Result<LoopTapes, LowerError> {
    let t0 = period_tape(lo, slots, When::T0, hoisted)?;
    let mut prefix = Vec::new();
    for t in 1..peel {
        prefix.push(period_tape(lo, slots, When::Fixed(t), hoisted)?);
    }
    let body = period_tape(lo, slots, When::Body, hoisted)?;
    Ok(LoopTapes {
        t0,
        prefix,
        body,
        peel,
    })
}

/// One period's tape. At `t = 0` the seed section comes first: every `init` is
/// evaluated and stored before any body op runs, which is what makes a
/// pre-origin `LoadSeed` well defined no matter the declaration order.
fn period_tape(
    lo: &mut Lowering<'_>,
    slots: &[SlotId],
    when: When,
    hoisted: bool,
) -> Result<Tape, LowerError> {
    let mut b = TapeBuilder::new();

    if when == When::T0 {
        for &id in slots {
            let Some(init) = lo.plan.series_of(id).and_then(|s| s.init.clone()) else {
                continue;
            };
            let start = b.mark();
            let r = lo.lower_component(&mut b, id, &init, "init", When::Seed)?;
            b.emit(O::StoreSeed(id, r));
            b.range(id, start);
        }
    }

    for &id in slots {
        let Some(slot) = lo.plan.series_of(id) else {
            continue;
        };
        let (init, expr) = (slot.init.clone(), slot.expr.clone());
        let start = b.mark();
        let r = match (when, &init) {
            // `init` *is* the value at t = 0 (`01-ir.md` §2.7): the formula is
            // not evaluated there at all.
            (When::T0, Some(_)) => {
                let r = b.reg()?;
                b.emit(O::LoadSeed(r, id));
                r
            }
            _ => {
                let Some(expr) = expr else { continue };
                lo.lower_component(&mut b, id, &expr, "expr", when)?
            }
        };
        b.emit(if hoisted {
            O::StoreHoisted(id, r)
        } else {
            O::StoreCur(id, r)
        });
        b.range(id, start);
    }
    Ok(b.finish())
}
