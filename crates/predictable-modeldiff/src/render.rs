//! Rendering an IR [`Expr`] back to the canonical expression grammar of
//! `01-ir.md` §4.2.
//!
//! The diff reports *text* for a human and for an agent, but it never diffs
//! text: every string here is printed from the tree that was compared, so a
//! `before`/`after` pair cannot disagree with the classification beside it.
//!
//! Unlike `predictable-fmt`'s printer this one has no source text to consult,
//! so it invents no parentheses beyond the ones precedence requires. That is
//! the right rule for a diff: `(a * b) + c` and `a * b + c` are the same tree
//! and must not show up as a change.

use predictable_fmt::format_f64;
use predictable_ir::{AggOp, BinaryOp, Expr, LitValue, UnaryOp};

const P_IF: u8 = 0;
const P_OR: u8 = 1;
const P_AND: u8 = 2;
const P_CMP: u8 = 3;
const P_SUM: u8 = 4;
const P_PRODUCT: u8 = 5;
const P_UNARY: u8 = 6;
const P_POW: u8 = 7;
const P_ATOM: u8 = 8;

fn binop_prec(op: BinaryOp) -> u8 {
    match op {
        BinaryOp::Or => P_OR,
        BinaryOp::And => P_AND,
        BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
            P_CMP
        }
        BinaryOp::Add | BinaryOp::Sub => P_SUM,
        BinaryOp::Mul | BinaryOp::Div => P_PRODUCT,
        BinaryOp::Pow => P_POW,
    }
}

fn prec(e: &Expr) -> u8 {
    match e {
        Expr::If { .. } => P_IF,
        Expr::Binary { op, .. } => binop_prec(*op),
        Expr::Unary { .. } => P_UNARY,
        _ => P_ATOM,
    }
}

/// The spelling of an aggregate in the grammar.
pub fn agg_name(op: AggOp) -> &'static str {
    match op {
        AggOp::Sum => "sum",
        AggOp::SumKahan => "sum_kahan",
        AggOp::Npv => "npv",
        AggOp::First => "first",
        AggOp::Last => "last",
        AggOp::At => "at",
        AggOp::MaxOver => "max_over",
        AggOp::MinOver => "min_over",
        AggOp::CountWhile => "count_while",
    }
}

/// Render an expression in canonical form, with the minimum parenthesisation.
pub fn render(e: &Expr) -> String {
    let mut out = String::new();
    print(e, Ctx::Top, &mut out);
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    Top,
    /// `(binding power of the parent, this is the right operand)`.
    Operand(u8, bool),
    Unary,
}

fn needs_parens(e: &Expr, ctx: Ctx) -> bool {
    let p = prec(e);
    match ctx {
        Ctx::Top => false,
        Ctx::Unary => p < P_UNARY,
        Ctx::Operand(min, right) => p < min || (right && p == min) || (min == P_CMP && p == P_CMP),
    }
}

fn print(e: &Expr, ctx: Ctx, out: &mut String) {
    let paren = needs_parens(e, ctx);
    if paren {
        out.push('(');
    }
    print_bare(e, out);
    if paren {
        out.push(')');
    }
}

fn print_bare(e: &Expr, out: &mut String) {
    match e {
        Expr::Lit { dtype, value } => print_lit(dtype, value, out),
        Expr::Ref { name } => out.push_str(name),
        Expr::Lag { name, k } => {
            out.push_str(name);
            out.push_str("[t-");
            out.push_str(&k.to_string());
            out.push(']');
        }
        Expr::At { name, k } => {
            out.push_str(name);
            out.push('[');
            out.push_str(&k.to_string());
            out.push(']');
        }
        Expr::Unary { op, operand } => {
            out.push_str(match op {
                UnaryOp::Neg => "-",
                UnaryOp::Not => "not ",
            });
            print(operand, Ctx::Unary, out);
        }
        Expr::Binary { op, lhs, rhs } => {
            let p = binop_prec(*op);
            // `^` is right-associative: the tight side is the left one.
            let (lctx, rctx) = if *op == BinaryOp::Pow {
                (Ctx::Operand(p, true), Ctx::Operand(p, false))
            } else {
                (Ctx::Operand(p, false), Ctx::Operand(p, true))
            };
            print(lhs, lctx, out);
            out.push(' ');
            out.push_str(op.symbol());
            out.push(' ');
            print(rhs, rctx, out);
        }
        Expr::If {
            cond,
            then,
            otherwise,
        } => {
            out.push_str("if ");
            print(cond, Ctx::Top, out);
            out.push_str(" then ");
            print(then, Ctx::Top, out);
            out.push_str(" else ");
            print(otherwise, Ctx::Top, out);
        }
        Expr::Call { func, args } => {
            out.push_str(func);
            out.push('(');
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                // `retime(x, mid)`: the timing keyword is carried as a string
                // literal, and prints back bare.
                if predictable_ir::is_timing_op(func) && i == 1 {
                    if let Expr::Lit {
                        value: LitValue::Text(tag),
                        ..
                    } = arg
                    {
                        out.push_str(tag);
                        continue;
                    }
                }
                print(arg, Ctx::Top, out);
            }
            out.push(')');
        }
        Expr::Lookup { table, keys } => {
            out.push_str(table);
            out.push_str("@(");
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                print(key, Ctx::Top, out);
            }
            out.push(')');
        }
        Expr::Agg { op, value, pred } => {
            out.push_str(agg_name(*op));
            out.push('(');
            print(value, Ctx::Top, out);
            if let Some(p) = pred {
                out.push_str(", ");
                print(p, Ctx::Top, out);
            }
            out.push(')');
        }
    }
}

fn print_lit(dtype: &predictable_ir::DType, value: &LitValue, out: &mut String) {
    use predictable_ir::DType;
    match value {
        LitValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        LitValue::Int(i) => out.push_str(&i.to_string()),
        LitValue::Float(x) => out.push_str(&format_f64(*x)),
        LitValue::Text(s) => match dtype {
            DType::Str | DType::Date | DType::Enum(_) => {
                out.push('"');
                out.push_str(s);
                out.push('"');
            }
            _ => out.push_str(s),
        },
    }
}

/// Render a literal on its own — used for assumption values and table cells.
pub fn render_lit(value: &LitValue) -> String {
    match value {
        LitValue::Float(x) => format_f64(*x),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use predictable_ir::{DType, Expr};

    fn f(x: f64) -> Expr {
        Expr::f64(x)
    }

    #[test]
    fn precedence_parenthesises_only_where_it_must() {
        // (a + b) * c needs its parentheses; a + b * c does not.
        let lhs = Expr::binary(BinaryOp::Add, Expr::r#ref("a"), Expr::r#ref("b"));
        let e = Expr::binary(BinaryOp::Mul, lhs, Expr::r#ref("c"));
        assert_eq!(render(&e), "(a + b) * c");

        let rhs = Expr::binary(BinaryOp::Mul, Expr::r#ref("b"), Expr::r#ref("c"));
        let e = Expr::binary(BinaryOp::Add, Expr::r#ref("a"), rhs);
        assert_eq!(render(&e), "a + b * c");
    }

    #[test]
    fn subtraction_keeps_its_right_operand_grouped() {
        let rhs = Expr::binary(BinaryOp::Sub, Expr::r#ref("b"), Expr::r#ref("c"));
        let e = Expr::binary(BinaryOp::Sub, Expr::r#ref("a"), rhs);
        assert_eq!(render(&e), "a - (b - c)");
    }

    #[test]
    fn lags_lookups_aggregates_and_floats() {
        let e = Expr::Agg {
            op: AggOp::Npv,
            value: Box::new(Expr::Lag {
                name: "cf".into(),
                k: 1,
            }),
            pred: None,
        };
        assert_eq!(render(&e), "npv(cf[t-1])");

        let e = Expr::Lookup {
            table: "qx".into(),
            keys: vec![Expr::r#ref("age"), f(1.05)],
        };
        assert_eq!(render(&e), "qx@(age, 1.05)");
    }

    #[test]
    fn timing_keyword_prints_bare() {
        let e = Expr::Call {
            func: "retime".into(),
            args: vec![
                Expr::r#ref("x"),
                Expr::Lit {
                    dtype: DType::Str,
                    value: LitValue::Text("mid".into()),
                },
            ],
        };
        assert_eq!(render(&e), "retime(x, mid)");
    }
}
