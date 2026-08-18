//! Lowering `Expr` to ops (`03-engine.md` §3.3) and the three-tape peel (§5.2).
//!
//! The runtime never walks the `Expr` tree. Every expression is flattened here,
//! in reverse postorder, into a straight line of ops over virtual registers.
//! Three things happen during that walk that are decisions rather than
//! mechanics:
//!
//! **Time indexing is resolved statically.** The same expression lowers
//! differently depending on which peel region it is being lowered for: at `t = 0`
//! a `x[t-1]` is a `LoadSeed`, at `t = 1` with a global peel of 3 it may still be
//! one, and in the body it is always an in-range `LoadLag`. The kernel therefore
//! never tests `t - k < 0`.
//!
//! **Traps are masked, not branched.** `If` is a value conditional, so both arms
//! run. A `Div` or `Lookup` inside an arm carries the conjunction of the
//! enclosing conditions as a mask register, computed once per arm and only when
//! the arm actually contains a trapping op. The mask is derived syntactically
//! from the `If` chain — there is no dynamic predicate stack (`01-ir.md` §2.6).
//!
//! **CSE stays inside one component.** Identical subtrees within a single
//! expression share a register; across components they never do, because
//! `explain()` must be able to show each component's own arithmetic (§3.5).
//! Trapping and reducing nodes are excluded even within a component: merging two
//! `Div`s would merge their trap sites, and a trap must name the expression the
//! model author wrote.

use std::collections::BTreeMap;

use predictable_ir::{AggOp, BinaryOp, DType, Expr, LitValue, Timing, UnaryOp};
use predictable_plan::{Plan, SlotId, Space};

use crate::op::{CmpOp, Fn1, Fn2, FnN, Op, Reg, TableId, TimeField};
use crate::tape::{Site, TapeBuilder};
use crate::LowerError;

/// Which period a tape is being lowered for (`03-engine.md` §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    /// The `init` seed section of the `t = 0` tape.
    Seed,
    /// The `t = 0` body.
    T0,
    /// A peeled period `1 ..= peel - 1`, where some lags are still pre-origin.
    Fixed(u32),
    /// `t >= peel`: every lag is in range.
    Body,
}

impl When {
    /// The period this tape runs at, when it is known statically.
    fn t(self) -> Option<u32> {
        match self {
            When::Seed | When::T0 => Some(0),
            When::Fixed(t) => Some(t),
            When::Body => None,
        }
    }
}

/// The per-component state of a lowering walk.
#[derive(Debug)]
struct Frame {
    when: When,
    slot: SlotId,
    /// `expr` or `init` — the root of an `ExprPath` (`01-ir.md` §3.0.1).
    root: &'static str,
    /// Common subexpressions already materialised, with the mask they were
    /// evaluated under.
    cse: Vec<(Expr, Option<Reg>, Reg)>,
}

/// Everything lowering needs to know about the model, derived once from the
/// [`Plan`].
#[derive(Debug)]
pub(crate) struct Lowering<'a> {
    pub(crate) plan: &'a Plan,
    by_name: BTreeMap<String, SlotId>,
    pub(crate) tables: Vec<String>,
    table_index: BTreeMap<String, u32>,
}

impl<'a> Lowering<'a> {
    pub(crate) fn new(plan: &'a Plan) -> Lowering<'a> {
        let mut by_name = BTreeMap::new();
        for s in &plan.scalars {
            by_name.insert(s.info.name.clone(), s.info.id);
        }
        for s in &plan.permp {
            by_name.insert(s.info.name.clone(), s.info.id);
        }
        for s in &plan.series {
            by_name.insert(s.info.name.clone(), s.info.id);
        }
        Lowering {
            plan,
            by_name,
            tables: Vec::new(),
            table_index: BTreeMap::new(),
        }
    }

    fn slot_of(&self, name: &str) -> Result<SlotId, LowerError> {
        self.by_name
            .get(name)
            .copied()
            .ok_or_else(|| LowerError::UnknownName(name.to_string()))
    }

    fn table(&mut self, name: &str) -> TableId {
        if let Some(&id) = self.table_index.get(name) {
            return TableId(id);
        }
        let id = self.tables.len() as u32;
        self.tables.push(name.to_string());
        self.table_index.insert(name.to_string(), id);
        TableId(id)
    }

    fn space(&self, slot: SlotId) -> Space {
        self.plan.slot_refs[slot.index()].space
    }

    fn is_hoisted(&self, slot: SlotId) -> bool {
        self.plan.series_of(slot).is_some_and(|s| s.hoistable)
    }

    fn has_init(&self, slot: SlotId) -> bool {
        self.plan.series_of(slot).is_some_and(|s| s.init.is_some())
    }

    // -----------------------------------------------------------------
    // One component
    // -----------------------------------------------------------------

    /// Lower `expr` for `slot` at `when`, returning the register holding it.
    pub(crate) fn lower_component(
        &mut self,
        b: &mut TapeBuilder,
        slot: SlotId,
        expr: &Expr,
        root: &'static str,
        when: When,
    ) -> Result<Reg, LowerError> {
        let mut frame = Frame {
            when,
            slot,
            root,
            cse: Vec::new(),
        };
        let path = root.to_string();
        self.expr(b, &mut frame, expr, &path, None)
    }

    fn expr(
        &mut self,
        b: &mut TapeBuilder,
        f: &mut Frame,
        e: &Expr,
        path: &str,
        mask: Option<Reg>,
    ) -> Result<Reg, LowerError> {
        if cse_eligible(e) {
            // A load or a literal cannot trap, so the mask it was first
            // evaluated under is irrelevant and one register serves both arms
            // of an `If`. Everything else must match masks exactly.
            let any_mask = pure_load(e);
            if let Some((_, _, r)) = f
                .cse
                .iter()
                .find(|(other, m, _)| other == e && (any_mask || *m == mask))
            {
                return Ok(*r);
            }
        }
        let reg = self.expr_uncached(b, f, e, path, mask)?;
        if cse_eligible(e) {
            f.cse.push((e.clone(), mask, reg));
        }
        Ok(reg)
    }

    fn expr_uncached(
        &mut self,
        b: &mut TapeBuilder,
        f: &mut Frame,
        e: &Expr,
        path: &str,
        mask: Option<Reg>,
    ) -> Result<Reg, LowerError> {
        match e {
            Expr::Lit { dtype, value } => self.literal(b, dtype, value),
            Expr::Ref { name } => self.read_cur(b, f, name),
            Expr::Lag { name, k } => self.read_lag(b, f, name, *k, path),
            Expr::At { name, k } => self.read_at(b, name, *k),
            Expr::Unary { op, operand } => {
                let a = self.expr(b, f, operand, &child(path, "operand"), mask)?;
                let dst = b.reg()?;
                b.emit(match op {
                    UnaryOp::Neg => Op::Neg(dst, a),
                    UnaryOp::Not => Op::Not(dst, a),
                });
                Ok(dst)
            }
            Expr::Binary { op, lhs, rhs } => {
                let a = self.expr(b, f, lhs, &child(path, "lhs"), mask)?;
                let c = self.expr(b, f, rhs, &child(path, "rhs"), mask)?;
                let dst = b.reg()?;
                let op = match op {
                    BinaryOp::Add => Op::Add(dst, a, c),
                    BinaryOp::Sub => Op::Sub(dst, a, c),
                    BinaryOp::Mul => Op::Mul(dst, a, c),
                    BinaryOp::Div => Op::Div {
                        dst,
                        lhs: a,
                        rhs: c,
                        mask,
                        site: self.site(b, f, path, false),
                    },
                    BinaryOp::Pow => Op::Pow {
                        dst,
                        lhs: a,
                        rhs: c,
                        mask,
                        site: self.site(b, f, path, false),
                    },
                    BinaryOp::And => Op::And(dst, a, c),
                    BinaryOp::Or => Op::Or(dst, a, c),
                    BinaryOp::Eq => Op::Cmp(dst, CmpOp::Eq, a, c),
                    BinaryOp::Ne => Op::Cmp(dst, CmpOp::Ne, a, c),
                    BinaryOp::Lt => Op::Cmp(dst, CmpOp::Lt, a, c),
                    BinaryOp::Le => Op::Cmp(dst, CmpOp::Le, a, c),
                    BinaryOp::Gt => Op::Cmp(dst, CmpOp::Gt, a, c),
                    BinaryOp::Ge => Op::Cmp(dst, CmpOp::Ge, a, c),
                };
                b.emit(op);
                Ok(dst)
            }
            Expr::If {
                cond,
                then,
                otherwise,
            } => self.conditional(b, f, cond, then, otherwise, path, mask),
            Expr::Call { func, args } => self.call(b, f, func, args, path, mask),
            Expr::Lookup { table, keys } => {
                let table = self.table(table);
                let mut regs = Vec::with_capacity(keys.len());
                for (i, k) in keys.iter().enumerate() {
                    regs.push(self.expr(b, f, k, &child(path, &format!("key{i}")), mask)?);
                }
                let site = self.site(b, f, path, false);
                let dst = b.reg()?;
                b.emit(Op::Lookup {
                    dst,
                    table,
                    keys: regs,
                    mask,
                    site,
                });
                Ok(dst)
            }
            Expr::Agg { op, value, pred } => {
                self.aggregate(b, f, *op, value, pred.as_deref(), path)
            }
        }
    }

    /// `If`: evaluate both arms, then `Select`. The arms' trap masks are the
    /// only place the enclosing condition is used for anything but the select.
    #[allow(clippy::too_many_arguments)]
    fn conditional(
        &mut self,
        b: &mut TapeBuilder,
        f: &mut Frame,
        cond: &Expr,
        then: &Expr,
        otherwise: &Expr,
        path: &str,
        mask: Option<Reg>,
    ) -> Result<Reg, LowerError> {
        let c = self.expr(b, f, cond, &child(path, "cond"), mask)?;

        // Masks are materialised only for arms that can trap: an arm of pure
        // arithmetic needs no mask, and emitting one anyway would cost an op
        // per `If` in every model that has none.
        let then_mask = if traps(then) {
            Some(self.conjoin(b, mask, c, false)?)
        } else {
            mask
        };
        let else_mask = if traps(otherwise) {
            Some(self.conjoin(b, mask, c, true)?)
        } else {
            mask
        };

        let a = self.expr(b, f, then, &child(path, "then"), then_mask)?;
        let d = self.expr(b, f, otherwise, &child(path, "else"), else_mask)?;
        let dst = b.reg()?;
        b.emit(Op::Select(dst, c, a, d));
        Ok(dst)
    }

    /// `outer AND cond`, or `outer AND NOT cond` for the else arm.
    fn conjoin(
        &mut self,
        b: &mut TapeBuilder,
        outer: Option<Reg>,
        cond: Reg,
        negate: bool,
    ) -> Result<Reg, LowerError> {
        let c = if negate {
            let n = b.reg()?;
            b.emit(Op::Not(n, cond));
            n
        } else {
            cond
        };
        match outer {
            None => Ok(c),
            Some(o) => {
                let dst = b.reg()?;
                b.emit(Op::And(dst, o, c));
                Ok(dst)
            }
        }
    }

    fn aggregate(
        &mut self,
        b: &mut TapeBuilder,
        f: &mut Frame,
        op: AggOp,
        value: &Expr,
        pred: Option<&Expr>,
        path: &str,
    ) -> Result<Reg, LowerError> {
        // `Reduce`/`Npv` read a whole retained series, so their operands must
        // name one. A computed operand (`sum(a * b)`) would need a materialised
        // temporary series, which is a planner decision, not a lowering one.
        let series = self.series_operand(value, path, "value", f)?;
        let dst = b.reg()?;
        match op {
            AggOp::Npv => {
                let disc = match pred {
                    Some(p) => self.series_operand(p, path, "pred", f)?,
                    None => return Err(LowerError::NpvNeedsDiscount { slot: f.slot }),
                };
                let timing = self
                    .plan
                    .series_of(series)
                    .map(|s| s.timing)
                    .unwrap_or(Timing::End);
                b.emit(Op::Npv {
                    dst,
                    value: series,
                    disc,
                    timing,
                });
            }
            _ => {
                let pred = match pred {
                    Some(p) => Some(self.series_operand(p, path, "pred", f)?),
                    None => None,
                };
                b.emit(Op::Reduce {
                    dst,
                    agg: op,
                    series,
                    pred,
                });
            }
        }
        Ok(dst)
    }

    fn series_operand(
        &self,
        e: &Expr,
        path: &str,
        seg: &str,
        f: &Frame,
    ) -> Result<SlotId, LowerError> {
        match e {
            Expr::Ref { name } => {
                let id = self.slot_of(name)?;
                if self.space(id) != Space::Series {
                    return Err(LowerError::NotASeries {
                        name: name.clone(),
                        path: child(path, seg),
                    });
                }
                Ok(id)
            }
            _ => Err(LowerError::AggArgumentNotASeries {
                slot: f.slot,
                path: child(path, seg),
            }),
        }
    }

    // -----------------------------------------------------------------
    // Reads
    // -----------------------------------------------------------------

    fn read_cur(
        &mut self,
        b: &mut TapeBuilder,
        f: &mut Frame,
        name: &str,
    ) -> Result<Reg, LowerError> {
        let id = self.slot_of(name)?;
        let dst = b.reg()?;
        let op = match self.space(id) {
            Space::Scalar => Op::LoadScalar(dst, id),
            Space::PerMp => Op::LoadPerMp(dst, id),
            Space::Series => match TimeField::from_name(name) {
                // The timeline is computed, not stored: `t`, `policy_year` and
                // friends come out of the timeline, not a buffer (§5).
                Some(tf) if self.plan.info(id).kind.is_input() => Op::LoadTime(dst, tf),
                _ if self.is_hoisted(id) => Op::LoadHoistedCur(dst, id),
                _ => Op::LoadCur(dst, id),
            },
        };
        // A `Series` read from the seed section is the seed of the *other*
        // slot, not its `t = 0` value: at seed time nothing has been stored.
        let op = match (f.when, &op) {
            (When::Seed, Op::LoadCur(d, s)) if self.has_init(*s) => Op::LoadSeed(*d, *s),
            _ => op,
        };
        b.emit(op);
        Ok(dst)
    }

    fn read_lag(
        &mut self,
        b: &mut TapeBuilder,
        f: &mut Frame,
        name: &str,
        k: u32,
        path: &str,
    ) -> Result<Reg, LowerError> {
        let id = self.slot_of(name)?;
        if self.space(id) != Space::Series {
            // A lag on a non-series is a shape error the checker owns; treat
            // the read as the value itself rather than inventing storage.
            return self.read_cur(b, f, name);
        }
        let in_range = match f.when.t() {
            None => true,
            Some(t) => k <= t,
        };
        let dst = b.reg()?;
        if in_range {
            b.emit(if self.is_hoisted(id) {
                Op::LoadHoistedLag(dst, id, k)
            } else {
                Op::LoadLag(dst, id, k)
            });
            return Ok(dst);
        }
        // Below the origin: the slot's `init`, else the zero of its dtype
        // (`01-ir.md` §2.7). Either way the site is tagged `pre_origin_default`
        // so `explain()` can say where the number came from.
        // Interned even though no op field points at it: it is the
        // `pre_origin_default` note §3.3 requires, and `explain()` finds it by
        // `(slot, path)` — the same key a trap would use.
        let _pre_origin_site = self.site(b, f, path, true);
        if self.has_init(id) {
            b.emit(Op::LoadSeed(dst, id));
            return Ok(dst);
        }
        let dtype = self.plan.info(id).dtype.clone();
        match dtype {
            DType::F64 => {
                let c = b.const_f(0.0);
                b.emit(Op::ConstF(dst, c));
            }
            DType::I64 => {
                let c = b.const_i(0);
                b.emit(Op::ConstI(dst, c));
            }
            DType::Bool => {
                let c = b.const_i(0);
                b.emit(Op::ConstI(dst, c));
            }
            _ => {
                return Err(LowerError::NoPreOriginValue {
                    name: name.to_string(),
                    dtype: dtype.to_string(),
                })
            }
        }
        Ok(dst)
    }

    fn read_at(&mut self, b: &mut TapeBuilder, name: &str, k: u32) -> Result<Reg, LowerError> {
        let id = self.slot_of(name)?;
        let dst = b.reg()?;
        b.emit(if self.is_hoisted(id) {
            Op::LoadHoistedAt(dst, id, k)
        } else {
            Op::LoadAt(dst, id, k)
        });
        Ok(dst)
    }

    fn literal(
        &mut self,
        b: &mut TapeBuilder,
        dtype: &DType,
        value: &LitValue,
    ) -> Result<Reg, LowerError> {
        let dst = b.reg()?;
        let _ = dtype;
        let op = match value {
            LitValue::Float(x) => Op::ConstF(dst, b.const_f(*x)),
            // An integer literal is a *number*, and the lanes arithmetic runs
            // in are `f64`. `1 - q` must not drag the expression into an
            // integer lane just because the author wrote `1` instead of `1.0`,
            // so an exactly-representable integer lowers to `ConstF`. Beyond
            // 2^53 the conversion would lose bits, and there it stays `ConstI`.
            LitValue::Int(i) if i.unsigned_abs() <= (1u64 << 53) => {
                Op::ConstF(dst, b.const_f(*i as f64))
            }
            LitValue::Int(i) => Op::ConstI(dst, b.const_i(*i)),
            LitValue::Bool(x) => Op::ConstI(dst, b.const_i(i64::from(*x))),
            LitValue::Text(s) => Op::ConstS(dst, b.const_s(s)),
        };
        b.emit(op);
        Ok(dst)
    }

    fn site(
        &self,
        b: &mut TapeBuilder,
        f: &Frame,
        path: &str,
        pre_origin_default: bool,
    ) -> crate::op::SiteId {
        let _ = f.root;
        b.site(Site {
            slot: f.slot,
            path: path.to_string(),
            pre_origin_default,
        })
    }

    // -----------------------------------------------------------------
    // Calls
    // -----------------------------------------------------------------

    fn call(
        &mut self,
        b: &mut TapeBuilder,
        f: &mut Frame,
        func: &str,
        args: &[Expr],
        path: &str,
        mask: Option<Reg>,
    ) -> Result<Reg, LowerError> {
        let arg = |i: usize| -> Result<&Expr, LowerError> {
            args.get(i).ok_or_else(|| LowerError::BadArity {
                func: func.to_string(),
                got: args.len(),
            })
        };

        // The timing builtins of §2.5 are series-level, not lanewise: they are
        // re-expressed here in terms of time-indexed loads rather than given
        // ops of their own, which keeps the kernel's op table small and keeps
        // `shift`/`diff` bit-identical to the lag they are sugar for.
        match func {
            "shift" => {
                let name = ref_name(arg(0)?, func)?;
                let k = int_lit(arg(1)?, func)?;
                if k < 0 {
                    return Err(LowerError::ForwardShift { slot: f.slot, k });
                }
                return if k == 0 {
                    self.read_cur(b, f, &name)
                } else {
                    self.read_lag(b, f, &name, k as u32, path)
                };
            }
            "retime" => {
                // Timing is a tag, not a value: `retime` changes what `npv`
                // does with the series, never the number in the lane. So it
                // lowers to its own argument, whatever that argument is —
                // including `retime(sum(x), end)`, which W0105 warns about but
                // which is still a legal program.
                return self.expr(b, f, arg(0)?, &child(path, "arg0"), mask);
            }
            "diff" => {
                let name = ref_name(arg(0)?, func)?;
                let cur = self.read_cur(b, f, &name)?;
                let prev = self.read_lag(b, f, &name, 1, path)?;
                let dst = b.reg()?;
                b.emit(Op::Sub(dst, cur, prev));
                return Ok(dst);
            }
            "cum" => {
                let name = ref_name(arg(0)?, func)?;
                let id = self.slot_of(&name)?;
                let acc = b.accumulator();
                let dst = b.reg()?;
                b.emit(Op::Cum(dst, id, acc));
                return Ok(dst);
            }
            "pow" => {
                let a = self.expr(b, f, arg(0)?, &child(path, "arg0"), mask)?;
                let c = self.expr(b, f, arg(1)?, &child(path, "arg1"), mask)?;
                let site = self.site(b, f, path, false);
                let dst = b.reg()?;
                b.emit(Op::Pow {
                    dst,
                    lhs: a,
                    rhs: c,
                    mask,
                    site,
                });
                return Ok(dst);
            }
            _ => {}
        }

        let mut regs = Vec::with_capacity(args.len());
        for (i, a) in args.iter().enumerate() {
            regs.push(self.expr(b, f, a, &child(path, &format!("arg{i}")), mask)?);
        }
        let dst = b.reg()?;
        let need = |n: usize| -> Result<(), LowerError> {
            if regs.len() == n {
                Ok(())
            } else {
                Err(LowerError::BadArity {
                    func: func.to_string(),
                    got: regs.len(),
                })
            }
        };

        if let Some(op) = cmp_call(func) {
            need(2)?;
            b.emit(Op::Cmp(dst, op, regs[0], regs[1]));
            return Ok(dst);
        }
        match func {
            "and" | "or" => {
                need(2)?;
                b.emit(if func == "and" {
                    Op::And(dst, regs[0], regs[1])
                } else {
                    Op::Or(dst, regs[0], regs[1])
                });
            }
            "not" => {
                need(1)?;
                b.emit(Op::Not(dst, regs[0]));
            }
            "clamp" => {
                need(3)?;
                b.emit(Op::CallN(dst, FnN::Clamp, regs.clone()));
            }
            "year_frac" => {
                need(3)?;
                b.emit(Op::CallN(dst, FnN::YearFrac, regs.clone()));
            }
            _ => {
                if let Some(fun) = fn1(func) {
                    need(1)?;
                    b.emit(Op::Call1(dst, fun, regs[0]));
                } else if let Some(fun) = fn2(func) {
                    need(2)?;
                    b.emit(Op::Call2(dst, fun, regs[0], regs[1]));
                } else {
                    return Err(LowerError::UnsupportedCall(func.to_string()));
                }
            }
        }
        Ok(dst)
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn child(path: &str, seg: &str) -> String {
    format!("{path}.{seg}")
}

fn ref_name(e: &Expr, func: &str) -> Result<String, LowerError> {
    match e {
        Expr::Ref { name } => Ok(name.clone()),
        _ => Err(LowerError::NeedsSeriesArgument(func.to_string())),
    }
}

fn int_lit(e: &Expr, func: &str) -> Result<i64, LowerError> {
    match e {
        Expr::Lit {
            value: LitValue::Int(k),
            ..
        } => Ok(*k),
        _ => Err(LowerError::NeedsLiteralArgument(func.to_string())),
    }
}

fn cmp_call(name: &str) -> Option<CmpOp> {
    Some(match name {
        "eq" => CmpOp::Eq,
        "ne" => CmpOp::Ne,
        "lt" => CmpOp::Lt,
        "le" => CmpOp::Le,
        "gt" => CmpOp::Gt,
        "ge" => CmpOp::Ge,
        _ => return None,
    })
}

fn fn1(name: &str) -> Option<Fn1> {
    Some(match name {
        "abs" => Fn1::Abs,
        "floor" => Fn1::Floor,
        "ceil" => Fn1::Ceil,
        "sign" => Fn1::Sign,
        "exp" => Fn1::Exp,
        "ln" => Fn1::Ln,
        "sqrt" => Fn1::Sqrt,
        "to_monthly" => Fn1::ToMonthly,
        "to_annual" => Fn1::ToAnnual,
        "v_from_i" => Fn1::VFromI,
        "i_from_v" => Fn1::IFromV,
        "year" => Fn1::Year,
        "month" => Fn1::Month,
        "day" => Fn1::Day,
        "is_null" => Fn1::IsNull,
        _ => return None,
    })
}

fn fn2(name: &str) -> Option<Fn2> {
    Some(match name {
        "min" => Fn2::Min,
        "max" => Fn2::Max,
        "round" => Fn2::Round,
        "nominal_to_periodic" => Fn2::NominalToPeriodic,
        "annuity_factor" => Fn2::AnnuityFactor,
        "compound" => Fn2::Compound,
        "coalesce" => Fn2::Coalesce,
        "add_months" => Fn2::AddMonths,
        "months_between" => Fn2::MonthsBetween,
        _ => return None,
    })
}

/// Does this subtree contain a trapping op? Only these arms need a mask.
pub(crate) fn traps(e: &Expr) -> bool {
    match e {
        Expr::Binary { op, lhs, rhs } => {
            matches!(op, BinaryOp::Div | BinaryOp::Pow) || traps(lhs) || traps(rhs)
        }
        Expr::Lookup { .. } => true,
        Expr::Unary { operand, .. } => traps(operand),
        Expr::If {
            cond,
            then,
            otherwise,
        } => traps(cond) || traps(then) || traps(otherwise),
        Expr::Call { func, args } => func == "pow" || args.iter().any(traps),
        Expr::Agg { value, pred, .. } => traps(value) || pred.as_deref().is_some_and(traps),
        _ => false,
    }
}

/// Nodes that may be shared by CSE: pure, non-trapping, non-reducing, and worth
/// a map lookup (a bare literal or load is cheaper to re-emit than to search).
fn cse_eligible(e: &Expr) -> bool {
    match e {
        Expr::Agg { .. } | Expr::Lookup { .. } => false,
        _ => !traps(e),
    }
}

/// A node whose value is a single load or constant: pure, cheap to name, and
/// insensitive to the trap mask.
fn pure_load(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Lit { .. } | Expr::Ref { .. } | Expr::Lag { .. } | Expr::At { .. }
    )
}
