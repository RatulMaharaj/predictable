//! `03-engine.md` §3.5 — what the planner folds, and what it refuses to.
//!
//! The refusals matter more than the folds. A planner that folds `x * 1.0` is
//! faster and wrong: it changes a bit pattern in the one case anybody would
//! ever notice (a signed zero flowing into a division), and it does it silently,
//! in a build where nothing in the model diff changed.

use predictable_ir::{BinaryOp, DType, Expr, LitValue, UnaryOp};
use predictable_plan::fold::fold;

fn f(x: f64) -> Expr {
    Expr::f64(x)
}

fn i(v: i64) -> Expr {
    Expr::Lit {
        dtype: DType::I64,
        value: LitValue::Int(v),
    }
}

fn b(v: bool) -> Expr {
    Expr::Lit {
        dtype: DType::Bool,
        value: LitValue::Bool(v),
    }
}

fn bin(op: BinaryOp, lhs: Expr, rhs: Expr) -> Expr {
    Expr::binary(op, lhs, rhs)
}

fn as_f64(e: &Expr) -> Option<f64> {
    match e {
        Expr::Lit {
            value: LitValue::Float(x),
            ..
        } => Some(*x),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// what folds
// ---------------------------------------------------------------------------

#[test]
fn two_literals_fold_to_the_ieee_result() {
    assert_eq!(
        as_f64(&fold(&bin(BinaryOp::Add, f(0.1), f(0.2)))),
        Some(0.1 + 0.2)
    );
    assert_eq!(
        as_f64(&fold(&bin(BinaryOp::Mul, f(2.0), f(3.0)))),
        Some(6.0)
    );
    assert_eq!(
        as_f64(&fold(&bin(BinaryOp::Div, f(1.0), f(4.0)))),
        Some(0.25)
    );
    // Not the decimal answer — the `f64` answer, which is the whole point.
    assert_ne!(
        as_f64(&fold(&bin(BinaryOp::Add, f(0.1), f(0.2)))),
        Some(0.3)
    );
}

#[test]
fn folding_is_bottom_up_and_reaches_nested_subtrees() {
    // `x * ((2.0 + 3.0) * 2.0)` → `x * 10.0`
    let expr = bin(
        BinaryOp::Mul,
        Expr::r#ref("x"),
        bin(BinaryOp::Mul, bin(BinaryOp::Add, f(2.0), f(3.0)), f(2.0)),
    );
    let folded = fold(&expr);
    match &folded {
        Expr::Binary { rhs, .. } => assert_eq!(as_f64(rhs), Some(10.0)),
        other => panic!("{other:?}"),
    }
    assert_eq!(predictable_plan::fold::size(&folded), 3);
}

#[test]
fn integers_fold_only_when_they_do_not_overflow() {
    assert_eq!(fold(&bin(BinaryOp::Add, i(2), i(3))), i(5));
    let overflow = bin(BinaryOp::Mul, i(i64::MAX), i(2));
    assert_eq!(fold(&overflow), overflow, "an overflowing fold is refused");
}

#[test]
fn literal_comparisons_and_logic_fold() {
    assert_eq!(fold(&bin(BinaryOp::Lt, f(1.0), f(2.0))), b(true));
    assert_eq!(fold(&bin(BinaryOp::And, b(true), b(false))), b(false));
    assert_eq!(fold(&bin(BinaryOp::Or, b(true), b(false))), b(true));
    assert_eq!(
        fold(&Expr::Unary {
            op: UnaryOp::Not,
            operand: Box::new(b(false))
        }),
        b(true)
    );
}

#[test]
fn negation_of_a_literal_keeps_the_sign_of_zero() {
    let neg_zero = fold(&Expr::Unary {
        op: UnaryOp::Neg,
        operand: Box::new(f(0.0)),
    });
    assert_eq!(as_f64(&neg_zero).unwrap().to_bits(), (-0.0f64).to_bits());
}

#[test]
fn a_literal_condition_selects_its_arm() {
    let expr = Expr::If {
        cond: Box::new(b(true)),
        then: Box::new(Expr::r#ref("a")),
        otherwise: Box::new(Expr::r#ref("b")),
    };
    assert_eq!(fold(&expr), Expr::r#ref("a"));
}

// ---------------------------------------------------------------------------
// what does not fold — the load-bearing half
// ---------------------------------------------------------------------------

#[test]
fn multiplying_by_one_is_not_removed() {
    // `-0.0 * 1.0` is `-0.0`, and `NaN * 1.0` is a NaN whose payload the
    // hardware chooses. Neither is `x`.
    let expr = bin(BinaryOp::Mul, Expr::r#ref("x"), f(1.0));
    assert_eq!(fold(&expr), expr);
}

#[test]
fn adding_zero_is_not_removed() {
    // `-0.0 + 0.0` is `+0.0`. Removing the add changes the sign of a zero, and
    // the sign of a zero decides the sign of the infinity it later divides into.
    let expr = bin(BinaryOp::Add, Expr::r#ref("x"), f(0.0));
    assert_eq!(fold(&expr), expr);
}

#[test]
fn subtracting_a_value_from_itself_is_not_zero() {
    // `inf - inf` is NaN.
    let expr = bin(BinaryOp::Sub, Expr::r#ref("x"), Expr::r#ref("x"));
    assert_eq!(fold(&expr), expr);
}

#[test]
fn division_by_a_literal_zero_is_left_to_the_kernel() {
    // §5.5: division traps at run time, with a span and a lane. Folding it here
    // would report it at plan time, without either.
    let expr = bin(BinaryOp::Div, f(1.0), f(0.0));
    assert_eq!(fold(&expr), expr);
}

#[test]
fn builtin_calls_are_never_folded() {
    // `round` is round-half-away-from-zero on the shortest decimal
    // (`01-ir.md` §2.8), not IEEE half-even. One implementation, in the kernel.
    let expr = Expr::Call {
        func: "round".to_string(),
        args: vec![f(2.675), i(2)],
    };
    assert_eq!(fold(&expr), expr);

    // `pow` is libm's, and §7 vendors exactly one libm.
    let p = bin(BinaryOp::Pow, f(1.05), f(3.0));
    assert_eq!(fold(&p), p);
}

#[test]
fn mixed_dtype_literals_are_not_folded() {
    // `2 * 3.0` mixes an `i64` and an `f64` literal. The conversion is the
    // checker's business (§7 pass 5) and the kernel's; the folder does not
    // invent one.
    let expr = bin(BinaryOp::Mul, i(2), f(3.0));
    assert_eq!(fold(&expr), expr);
}

#[test]
fn folding_is_idempotent() {
    let expr = bin(
        BinaryOp::Add,
        bin(BinaryOp::Mul, f(2.0), f(3.0)),
        Expr::r#ref("x"),
    );
    let once = fold(&expr);
    assert_eq!(fold(&once), once);
}
