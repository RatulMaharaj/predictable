//! Constant folding, restricted to rewrites that are exactly IEEE-preserving
//! (`03-engine.md` §3.5).
//!
//! The rule the whole pass is built around: **optimisation may change speed,
//! never bits.** So the identities a normal compiler would take are refused
//! here, and the refusals are the interesting part:
//!
//! | rewrite | folded? | why |
//! |---|---|---|
//! | `2.0 * 3.0` → `6.0` | yes | one IEEE multiply, evaluated now instead of later |
//! | `x * 1.0` → `x` | **no** | differs on `-0.0` (`-0.0 * 1.0` is `-0.0`, fine) and on NaN payloads; and `x` may be a signalling trap the mask must still see |
//! | `x + 0.0` → `x` | **no** | `-0.0 + 0.0` is `+0.0`, not `-0.0` |
//! | `x - x` → `0.0` | **no** | `inf - inf` is NaN |
//! | `x / 0.0` | **no** | division by zero traps at run time with a span (§5.5); folding would move the trap to plan time and lose the lane |
//! | `if true then a else b` → `a` | yes | `If` is a value conditional over pure arms (`01-ir.md` §2.6); a literal condition is the same value either way |
//! | `round(2.675, 2)` | **no** | builtin semantics are the kernel's (`01-ir.md` §2.8 pins round-half-away-from-zero on the shortest decimal); one implementation of each builtin, not two |
//!
//! Everything folded is a single IEEE operation on two literals, performed with
//! the same `f64` instruction the kernel would use, so `plan_opt ≡bits
//! plan_noopt` is a property of the construction rather than of the test — and
//! the test asserts it anyway, over the whole corpus, at [`crate`] level.

use predictable_ir::{BinaryOp, DType, Expr, LitValue, UnaryOp};

/// Fold an expression bottom-up. Pure: the input is untouched.
pub fn fold(expr: &Expr) -> Expr {
    match expr {
        Expr::Lit { .. } | Expr::Ref { .. } | Expr::Lag { .. } | Expr::At { .. } => expr.clone(),
        Expr::Unary { op, operand } => {
            let operand = fold(operand);
            if let Some(v) = fold_unary(*op, &operand) {
                return v;
            }
            Expr::Unary {
                op: *op,
                operand: Box::new(operand),
            }
        }
        Expr::Binary { op, lhs, rhs } => {
            let lhs = fold(lhs);
            let rhs = fold(rhs);
            if let Some(v) = fold_binary(*op, &lhs, &rhs) {
                return v;
            }
            Expr::Binary {
                op: *op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            }
        }
        Expr::If {
            cond,
            then,
            otherwise,
        } => {
            let cond = fold(cond);
            let then = fold(then);
            let otherwise = fold(otherwise);
            if let Expr::Lit {
                value: LitValue::Bool(b),
                ..
            } = cond
            {
                return if b { then } else { otherwise };
            }
            Expr::If {
                cond: Box::new(cond),
                then: Box::new(then),
                otherwise: Box::new(otherwise),
            }
        }
        // Builtin calls are never folded: `01-ir.md` §2.8 pins their semantics
        // (`round` is half-away-from-zero on the shortest decimal, not IEEE
        // half-even) and the kernel owns the one implementation of each.
        Expr::Call { func, args } => Expr::Call {
            func: func.clone(),
            args: args.iter().map(fold).collect(),
        },
        Expr::Lookup { table, keys } => Expr::Lookup {
            table: table.clone(),
            keys: keys.iter().map(fold).collect(),
        },
        Expr::Agg { op, value, pred } => Expr::Agg {
            op: *op,
            value: Box::new(fold(value)),
            pred: pred.as_ref().map(|p| Box::new(fold(p))),
        },
    }
}

fn as_f64(expr: &Expr) -> Option<f64> {
    match expr {
        Expr::Lit {
            dtype: DType::F64,
            value: LitValue::Float(x),
        } => Some(*x),
        _ => None,
    }
}

fn as_i64(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Lit {
            dtype: DType::I64,
            value: LitValue::Int(i),
        } => Some(*i),
        _ => None,
    }
}

fn as_bool(expr: &Expr) -> Option<bool> {
    match expr {
        Expr::Lit {
            dtype: DType::Bool,
            value: LitValue::Bool(b),
        } => Some(*b),
        _ => None,
    }
}

fn f64_lit(x: f64) -> Expr {
    Expr::Lit {
        dtype: DType::F64,
        value: LitValue::Float(x),
    }
}

fn i64_lit(i: i64) -> Expr {
    Expr::Lit {
        dtype: DType::I64,
        value: LitValue::Int(i),
    }
}

fn bool_lit(b: bool) -> Expr {
    Expr::Lit {
        dtype: DType::Bool,
        value: LitValue::Bool(b),
    }
}

fn fold_unary(op: UnaryOp, operand: &Expr) -> Option<Expr> {
    match op {
        // `-x` on a literal is a sign-bit flip, exact for every f64 including
        // zeroes and NaNs.
        UnaryOp::Neg => {
            if let Some(x) = as_f64(operand) {
                Some(f64_lit(-x))
            } else {
                as_i64(operand).and_then(|i| i.checked_neg()).map(i64_lit)
            }
        }
        UnaryOp::Not => as_bool(operand).map(|b| bool_lit(!b)),
    }
}

fn fold_binary(op: BinaryOp, lhs: &Expr, rhs: &Expr) -> Option<Expr> {
    if let (Some(a), Some(b)) = (as_f64(lhs), as_f64(rhs)) {
        return fold_f64(op, a, b);
    }
    if let (Some(a), Some(b)) = (as_i64(lhs), as_i64(rhs)) {
        return fold_i64(op, a, b);
    }
    if let (Some(a), Some(b)) = (as_bool(lhs), as_bool(rhs)) {
        // `and`/`or` do not short-circuit in the IR (§2.6: both arms are
        // evaluated), so folding two literals cannot skip a trap.
        return match op {
            BinaryOp::And => Some(bool_lit(a && b)),
            BinaryOp::Or => Some(bool_lit(a || b)),
            BinaryOp::Eq => Some(bool_lit(a == b)),
            BinaryOp::Ne => Some(bool_lit(a != b)),
            _ => None,
        };
    }
    None
}

fn fold_f64(op: BinaryOp, a: f64, b: f64) -> Option<Expr> {
    Some(match op {
        BinaryOp::Add => f64_lit(a + b),
        BinaryOp::Sub => f64_lit(a - b),
        BinaryOp::Mul => f64_lit(a * b),
        // Left for the kernel: `x / 0.0` is a trap with a span (§5.5), and a
        // plan-time fold would report it without a modelpoint or a lane.
        BinaryOp::Div if b == 0.0 => return None,
        BinaryOp::Div => f64_lit(a / b),
        // `powf` is libm, and §7 vendors one libm for determinism; the kernel
        // calls it, the planner does not get a second opinion.
        BinaryOp::Pow => return None,
        BinaryOp::Eq => bool_lit(a == b),
        BinaryOp::Ne => bool_lit(a != b),
        BinaryOp::Lt => bool_lit(a < b),
        BinaryOp::Le => bool_lit(a <= b),
        BinaryOp::Gt => bool_lit(a > b),
        BinaryOp::Ge => bool_lit(a >= b),
        BinaryOp::And | BinaryOp::Or => return None,
    })
}

fn fold_i64(op: BinaryOp, a: i64, b: i64) -> Option<Expr> {
    Some(match op {
        BinaryOp::Add => i64_lit(a.checked_add(b)?),
        BinaryOp::Sub => i64_lit(a.checked_sub(b)?),
        BinaryOp::Mul => i64_lit(a.checked_mul(b)?),
        // Integer division is the kernel's (it traps on zero and its rounding is
        // a documented semantic), and `pow` is libm's.
        BinaryOp::Div | BinaryOp::Pow => return None,
        BinaryOp::Eq => bool_lit(a == b),
        BinaryOp::Ne => bool_lit(a != b),
        BinaryOp::Lt => bool_lit(a < b),
        BinaryOp::Le => bool_lit(a <= b),
        BinaryOp::Gt => bool_lit(a > b),
        BinaryOp::Ge => bool_lit(a >= b),
        BinaryOp::And | BinaryOp::Or => return None,
    })
}

/// How many nodes `fold` removed — reported by the planner so `--O0` and `--O1`
/// are comparable without diffing trees.
pub fn size(expr: &Expr) -> usize {
    1 + match expr {
        Expr::Lit { .. } | Expr::Ref { .. } | Expr::Lag { .. } | Expr::At { .. } => 0,
        Expr::Unary { operand, .. } => size(operand),
        Expr::Binary { lhs, rhs, .. } => size(lhs) + size(rhs),
        Expr::If {
            cond,
            then,
            otherwise,
        } => size(cond) + size(then) + size(otherwise),
        Expr::Call { args, .. } => args.iter().map(size).sum(),
        Expr::Lookup { keys, .. } => keys.iter().map(size).sum(),
        Expr::Agg { value, pred, .. } => size(value) + pred.as_ref().map(|p| size(p)).unwrap_or(0),
    }
}
