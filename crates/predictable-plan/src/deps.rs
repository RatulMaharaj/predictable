//! Reading the dependency edges of `01-ir.md` §3 out of an [`Expr`].
//!
//! One walk, one edge list. The planner needs four facts about each reference
//! that a checker does not: whether it was read at lag 0 (which is what
//! constrains evaluation order *within* a period), how far back the largest lag
//! reaches (which sizes the ring buffer), whether it was read by an `At` (which
//! forces `Full` retention), and whether it sits under an `Agg` (which makes the
//! reading component stage 2 and the read slot a reduce target).

use predictable_ir::Expr;

/// How a name was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Read {
    /// `x` — the same period.
    Cur,
    /// `x[t-k]`, `k >= 1`.
    Lag(u32),
    /// `x[k]` — an absolute period.
    At(u32),
    /// `tbl@(..)` — the table namespace, never the value namespace.
    Table,
}

/// One reference found in an expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    pub name: String,
    pub read: Read,
    /// True when the reference is under an `Agg` node: a whole-series edge,
    /// `stage = 2`, and therefore *not* in `G₀` (`01-ir.md` §3.1).
    pub in_agg: bool,
}

impl Ref {
    /// True when this edge constrains order within a single period: lag 0,
    /// stage 1. `Lag` edges are unconstrained (last period is already data) and
    /// `At` edges are seeds.
    pub fn is_instantaneous(&self) -> bool {
        self.read == Read::Cur && !self.in_agg
    }
}

/// Every reference in `expr`, in a deterministic pre-order walk.
pub fn refs(expr: &Expr) -> Vec<Ref> {
    let mut out = Vec::new();
    walk(expr, false, &mut out);
    out
}

/// Every reference across an optional `expr` and an optional `init`.
pub fn refs_of(expr: Option<&Expr>, init: Option<&Expr>) -> Vec<Ref> {
    let mut out = Vec::new();
    if let Some(e) = expr {
        walk(e, false, &mut out);
    }
    if let Some(e) = init {
        walk(e, false, &mut out);
    }
    out
}

fn walk(expr: &Expr, in_agg: bool, out: &mut Vec<Ref>) {
    match expr {
        Expr::Lit { .. } => {}
        Expr::Ref { name } => out.push(Ref {
            name: name.clone(),
            read: Read::Cur,
            in_agg,
        }),
        Expr::Lag { name, k } => out.push(Ref {
            name: name.clone(),
            read: Read::Lag(*k),
            in_agg,
        }),
        Expr::At { name, k } => out.push(Ref {
            name: name.clone(),
            read: Read::At(*k),
            in_agg,
        }),
        Expr::Unary { operand, .. } => walk(operand, in_agg, out),
        Expr::Binary { lhs, rhs, .. } => {
            walk(lhs, in_agg, out);
            walk(rhs, in_agg, out);
        }
        Expr::If {
            cond,
            then,
            otherwise,
        } => {
            walk(cond, in_agg, out);
            walk(then, in_agg, out);
            walk(otherwise, in_agg, out);
        }
        Expr::Call { args, .. } => {
            for a in args {
                walk(a, in_agg, out);
            }
        }
        Expr::Lookup { table, keys } => {
            out.push(Ref {
                name: table.clone(),
                read: Read::Table,
                in_agg,
            });
            for k in keys {
                walk(k, in_agg, out);
            }
        }
        Expr::Agg { value, pred, .. } => {
            walk(value, true, out);
            if let Some(p) = pred {
                walk(p, true, out);
            }
        }
    }
}
