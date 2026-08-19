//! The `Op` set of `03-engine.md` §3.3.
//!
//! An op is a *whole-lane* instruction: the kernel applies it to all `C`
//! modelpoints of a chunk in one pass, so dispatch cost is amortised 1024× and
//! the arithmetic stays lane-independent (§4.2). Nothing here branches on data.
//!
//! Three shapes of decision are baked into the op set and are worth naming:
//!
//! * **`Select`, never a jump.** `If` evaluates both arms (`01-ir.md` §2.6), so
//!   the tape has no control flow at all — it is a straight line from the first
//!   op to the last. Trapping ops inside an arm carry a `mask` register instead;
//!   see [`Op::mask`].
//! * **Time indexing is resolved at lowering, not at run time.** There is a
//!   distinct op for `x[t]`, `x[t-k]`, `x[k]` and for the pre-origin seed, and
//!   which one appears is decided by the peel region the tape belongs to
//!   (§5.2). The kernel never asks "is `t - k` below the origin?".
//! * **Hoisted series are a different load.** A loop-invariant series lives in a
//!   shared `(T+1)` array with no modelpoint axis, so reading it is a stride-0
//!   broadcast (`LoadHoisted*`) rather than a chunk-buffer read (§3.4).

use std::fmt;

use predictable_ir::{AggOp, Timing};
use predictable_plan::SlotId;
use serde::{Deserialize, Serialize};

/// A scratch register: an index into the per-chunk `C × n_regs` frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Reg(pub u16);

impl fmt::Display for Reg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "r{}", self.0)
    }
}

/// An index into one of a [`crate::Tape`]'s constant pools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ConstId(pub u32);

/// An index into [`crate::TapeProgram::tables`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TableId(pub u32);

/// An index into [`crate::Tape::sites`] — the provenance a trap or an
/// `explain()` node is reported against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SiteId(pub u32);

/// A running-total accumulator, one per `cum()` in a tape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AccId(pub u16);

/// The timeline fields of `01-ir.md` §5, computed by the kernel from the
/// timeline rather than read from a buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeField {
    T,
    PeriodStartDate,
    PeriodEndDate,
    YearFrac,
    MonthOfYear,
    PolicyYear,
    PolicyMonth,
    IsAnniversary,
}

impl TimeField {
    /// The `01-ir.md` §5 name of the field, and the reverse mapping.
    pub fn name(self) -> &'static str {
        match self {
            TimeField::T => "t",
            TimeField::PeriodStartDate => "period_start_date",
            TimeField::PeriodEndDate => "period_end_date",
            TimeField::YearFrac => "year_frac",
            TimeField::MonthOfYear => "month_of_year",
            TimeField::PolicyYear => "policy_year",
            TimeField::PolicyMonth => "policy_month",
            TimeField::IsAnniversary => "is_anniversary",
        }
    }

    pub fn from_name(name: &str) -> Option<TimeField> {
        Some(match name {
            "t" => TimeField::T,
            "period_start_date" => TimeField::PeriodStartDate,
            "period_end_date" => TimeField::PeriodEndDate,
            "year_frac" => TimeField::YearFrac,
            "month_of_year" => TimeField::MonthOfYear,
            "policy_year" => TimeField::PolicyYear,
            "policy_month" => TimeField::PolicyMonth,
            "is_anniversary" => TimeField::IsAnniversary,
            _ => return None,
        })
    }
}

/// Comparison, split out of `BinaryOp` because the kernel implements one op
/// with a mode rather than six ops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CmpOp {
    pub fn symbol(self) -> &'static str {
        match self {
            CmpOp::Eq => "==",
            CmpOp::Ne => "!=",
            CmpOp::Lt => "<",
            CmpOp::Le => "<=",
            CmpOp::Gt => ">",
            CmpOp::Ge => ">=",
        }
    }
}

/// Unary builtins (`01-ir.md` §2.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fn1 {
    Abs,
    Floor,
    Ceil,
    Sign,
    Exp,
    Ln,
    Sqrt,
    ToMonthly,
    ToAnnual,
    VFromI,
    IFromV,
    Year,
    Month,
    Day,
    IsNull,
}

/// Binary builtins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fn2 {
    Min,
    Max,
    Round,
    NominalToPeriodic,
    AnnuityFactor,
    Compound,
    Coalesce,
    AddMonths,
    MonthsBetween,
}

/// Ternary-and-wider builtins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FnN {
    Clamp,
    YearFrac,
}

/// One instruction.
///
/// `Div`, `Pow` and `Lookup` are the trapping ops (`01-ir.md` §9.3) and are the
/// only ones carrying a [`SiteId`] and a `mask`. `mask = Some(r)` is the
/// spec's `DivMasked`: the trap is raised only on lanes where `r` is true, which
/// is what makes `if x == 0 then 0.0 else 1.0 / x` legal (§2.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    ConstF(Reg, ConstId),
    ConstI(Reg, ConstId),
    /// A `str`/`date`/`enum` literal, dictionary-encoded by the kernel.
    ConstS(Reg, ConstId),
    LoadScalar(Reg, SlotId),
    LoadPerMp(Reg, SlotId),
    /// `x[t]` out of the chunk buffer.
    LoadCur(Reg, SlotId),
    /// `x[t-k]`, guaranteed in range for the tape this op appears in.
    LoadLag(Reg, SlotId, u32),
    /// `x[k]`, absolute index into a `Retention::Full` slot.
    LoadAt(Reg, SlotId, u32),
    /// `x[t-k]` with `t - k < 0`: the slot's `init` seed (`01-ir.md` §2.7).
    /// Emitted only in the peeled tapes, where the planner proved it statically.
    LoadSeed(Reg, SlotId),
    /// Same three reads against the shared loop-invariant `(T+1)` array (§3.4).
    LoadHoistedCur(Reg, SlotId),
    LoadHoistedLag(Reg, SlotId, u32),
    LoadHoistedAt(Reg, SlotId, u32),
    LoadTime(Reg, TimeField),
    Add(Reg, Reg, Reg),
    Sub(Reg, Reg, Reg),
    Mul(Reg, Reg, Reg),
    Neg(Reg, Reg),
    Div {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
        mask: Option<Reg>,
        site: SiteId,
    },
    Pow {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
        mask: Option<Reg>,
        site: SiteId,
    },
    Cmp(Reg, CmpOp, Reg, Reg),
    And(Reg, Reg, Reg),
    Or(Reg, Reg, Reg),
    Not(Reg, Reg),
    /// `dst = if cond then a else b`, both arms already evaluated.
    Select(Reg, Reg, Reg, Reg),
    Call1(Reg, Fn1, Reg),
    Call2(Reg, Fn2, Reg, Reg),
    CallN(Reg, FnN, Vec<Reg>),
    Lookup {
        dst: Reg,
        table: TableId,
        keys: Vec<Reg>,
        mask: Option<Reg>,
        site: SiteId,
    },
    /// `cum(x)` — the running total of a series, kept in a tape accumulator.
    Cum(Reg, SlotId, AccId),
    /// Commit `x[t]`.
    StoreCur(SlotId, Reg),
    /// Commit the `t = 0` seed of a series with an `init` (§5.2).
    StoreSeed(SlotId, Reg),
    StoreHoisted(SlotId, Reg),
    StorePerMp(SlotId, Reg),
    StoreScalar(SlotId, Reg),
    /// Stage 2 only: a reduction over a whole retained series, strictly
    /// sequential in `t` (§3.3). `pred` is the second `Agg` operand.
    Reduce {
        dst: Reg,
        agg: AggOp,
        series: SlotId,
        pred: Option<SlotId>,
    },
    /// Stage 2 only: `npv(value, disc)` using `value`'s timing tag.
    Npv {
        dst: Reg,
        value: SlotId,
        disc: SlotId,
        timing: Timing,
    },
}

/// Whether a register position on an op is written or read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegRole {
    Def,
    Use,
}

impl Op {
    /// The register this op writes, if any.
    pub fn def(&self) -> Option<Reg> {
        let mut found = None;
        self.for_each_reg(|r, role| {
            if role == RegRole::Def {
                found = Some(r);
            }
        });
        found
    }

    /// The trap mask, if this op is a masked trapping op.
    pub fn mask(&self) -> Option<Reg> {
        match self {
            Op::Div { mask, .. } | Op::Pow { mask, .. } | Op::Lookup { mask, .. } => *mask,
            _ => None,
        }
    }

    /// True for the ops that can raise a trap (`01-ir.md` §9.3).
    pub fn traps(&self) -> bool {
        matches!(self, Op::Div { .. } | Op::Pow { .. } | Op::Lookup { .. })
    }

    /// The provenance site of a trapping op.
    pub fn site(&self) -> Option<SiteId> {
        match self {
            Op::Div { site, .. } | Op::Pow { site, .. } | Op::Lookup { site, .. } => Some(*site),
            _ => None,
        }
    }

    pub fn for_each_reg(&self, mut f: impl FnMut(Reg, RegRole)) {
        let mut this = self.clone();
        this.for_each_reg_mut(|r, role| f(*r, role));
    }

    /// Visit every register slot, in `def`-then-`use` order. This is the single
    /// definition of an op's register interface: the allocator, the printer and
    /// the validator all go through it, so adding an op cannot desynchronise
    /// them.
    pub fn for_each_reg_mut(&mut self, mut f: impl FnMut(&mut Reg, RegRole)) {
        use RegRole::{Def, Use};
        match self {
            Op::ConstF(d, _)
            | Op::ConstI(d, _)
            | Op::ConstS(d, _)
            | Op::LoadScalar(d, _)
            | Op::LoadPerMp(d, _)
            | Op::LoadCur(d, _)
            | Op::LoadLag(d, _, _)
            | Op::LoadAt(d, _, _)
            | Op::LoadSeed(d, _)
            | Op::LoadHoistedCur(d, _)
            | Op::LoadHoistedLag(d, _, _)
            | Op::LoadHoistedAt(d, _, _)
            | Op::LoadTime(d, _)
            | Op::Cum(d, _, _)
            | Op::Reduce { dst: d, .. }
            | Op::Npv { dst: d, .. } => f(d, Def),
            Op::Add(d, a, b) | Op::Sub(d, a, b) | Op::Mul(d, a, b) => {
                f(d, Def);
                f(a, Use);
                f(b, Use);
            }
            Op::And(d, a, b) | Op::Or(d, a, b) => {
                f(d, Def);
                f(a, Use);
                f(b, Use);
            }
            Op::Cmp(d, _, a, b) => {
                f(d, Def);
                f(a, Use);
                f(b, Use);
            }
            Op::Neg(d, a) | Op::Not(d, a) => {
                f(d, Def);
                f(a, Use);
            }
            Op::Div {
                dst,
                lhs,
                rhs,
                mask,
                ..
            }
            | Op::Pow {
                dst,
                lhs,
                rhs,
                mask,
                ..
            } => {
                f(dst, Def);
                f(lhs, Use);
                f(rhs, Use);
                if let Some(m) = mask {
                    f(m, Use);
                }
            }
            Op::Select(d, c, a, b) => {
                f(d, Def);
                f(c, Use);
                f(a, Use);
                f(b, Use);
            }
            Op::Call1(d, _, a) => {
                f(d, Def);
                f(a, Use);
            }
            Op::Call2(d, _, a, b) => {
                f(d, Def);
                f(a, Use);
                f(b, Use);
            }
            Op::CallN(d, _, args) => {
                f(d, Def);
                for a in args {
                    f(a, Use);
                }
            }
            Op::Lookup {
                dst, keys, mask, ..
            } => {
                f(dst, Def);
                for k in keys {
                    f(k, Use);
                }
                if let Some(m) = mask {
                    f(m, Use);
                }
            }
            Op::StoreCur(_, r)
            | Op::StoreSeed(_, r)
            | Op::StoreHoisted(_, r)
            | Op::StorePerMp(_, r)
            | Op::StoreScalar(_, r) => f(r, Use),
        }
    }
}

/// The canonical one-line rendering of an op. This is what the tape digest is
/// computed over and what the golden tests assert on, so it is a stable format,
/// not a `Debug` convenience.
impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn regs(rs: &[Reg]) -> String {
            rs.iter()
                .map(|r| r.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        }
        fn mask(m: &Option<Reg>) -> String {
            match m {
                Some(r) => format!(" mask {r}"),
                None => String::new(),
            }
        }
        match self {
            Op::ConstF(d, c) => write!(f, "{d} = constf #{}", c.0),
            Op::ConstI(d, c) => write!(f, "{d} = consti #{}", c.0),
            Op::ConstS(d, c) => write!(f, "{d} = consts #{}", c.0),
            Op::LoadScalar(d, s) => write!(f, "{d} = load.scalar {s}"),
            Op::LoadPerMp(d, s) => write!(f, "{d} = load.permp {s}"),
            Op::LoadCur(d, s) => write!(f, "{d} = load.cur {s}"),
            Op::LoadLag(d, s, k) => write!(f, "{d} = load.lag {s} {k}"),
            Op::LoadAt(d, s, k) => write!(f, "{d} = load.at {s} {k}"),
            Op::LoadSeed(d, s) => write!(f, "{d} = load.seed {s}"),
            Op::LoadHoistedCur(d, s) => write!(f, "{d} = load.hoisted.cur {s}"),
            Op::LoadHoistedLag(d, s, k) => write!(f, "{d} = load.hoisted.lag {s} {k}"),
            Op::LoadHoistedAt(d, s, k) => write!(f, "{d} = load.hoisted.at {s} {k}"),
            Op::LoadTime(d, tf) => write!(f, "{d} = load.time {}", tf.name()),
            Op::Add(d, a, b) => write!(f, "{d} = add {a}, {b}"),
            Op::Sub(d, a, b) => write!(f, "{d} = sub {a}, {b}"),
            Op::Mul(d, a, b) => write!(f, "{d} = mul {a}, {b}"),
            Op::Neg(d, a) => write!(f, "{d} = neg {a}"),
            Op::Div {
                dst,
                lhs,
                rhs,
                mask: m,
                site,
            } => write!(f, "{dst} = div {lhs}, {rhs}{} @{}", mask(m), site.0),
            Op::Pow {
                dst,
                lhs,
                rhs,
                mask: m,
                site,
            } => write!(f, "{dst} = pow {lhs}, {rhs}{} @{}", mask(m), site.0),
            Op::Cmp(d, op, a, b) => write!(f, "{d} = cmp {a} {} {b}", op.symbol()),
            Op::And(d, a, b) => write!(f, "{d} = and {a}, {b}"),
            Op::Or(d, a, b) => write!(f, "{d} = or {a}, {b}"),
            Op::Not(d, a) => write!(f, "{d} = not {a}"),
            Op::Select(d, c, a, b) => write!(f, "{d} = select {c} ? {a} : {b}"),
            Op::Call1(d, fun, a) => write!(f, "{d} = call1 {fun:?} {a}"),
            Op::Call2(d, fun, a, b) => write!(f, "{d} = call2 {fun:?} {a}, {b}"),
            Op::CallN(d, fun, args) => write!(f, "{d} = calln {fun:?} {}", regs(args)),
            Op::Lookup {
                dst,
                table,
                keys,
                mask: m,
                site,
            } => write!(
                f,
                "{dst} = lookup t{} ({}){} @{}",
                table.0,
                regs(keys),
                mask(m),
                site.0
            ),
            Op::Cum(d, s, acc) => write!(f, "{d} = cum {s} acc{}", acc.0),
            Op::StoreCur(s, r) => write!(f, "store.cur {s} <- {r}"),
            Op::StoreSeed(s, r) => write!(f, "store.seed {s} <- {r}"),
            Op::StoreHoisted(s, r) => write!(f, "store.hoisted {s} <- {r}"),
            Op::StorePerMp(s, r) => write!(f, "store.permp {s} <- {r}"),
            Op::StoreScalar(s, r) => write!(f, "store.scalar {s} <- {r}"),
            Op::Reduce {
                dst,
                agg,
                series,
                pred,
            } => match pred {
                Some(p) => write!(f, "{dst} = reduce {agg:?} {series} pred {p}"),
                None => write!(f, "{dst} = reduce {agg:?} {series}"),
            },
            Op::Npv {
                dst,
                value,
                disc,
                timing,
            } => write!(f, "{dst} = npv {value}, {disc} timing {timing}"),
        }
    }
}
