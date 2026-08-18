//! Lowering to the IR data model, and the `Call → Agg` rewrite in particular.
//!
//! The parser has no aggregate node — `npv(x, disc)` is a call, syntactically —
//! and the IR does, because an aggregate is what makes a component stage 2
//! (`01-ir.md` §2.2, §8.2). This is where the two meet.

use predictable_ir::{AggOp, Expr as IrExpr, Stage};
use predictable_syntax::{parse, SourceMap};

fn document(text: &str) -> predictable_syntax::PirDocument {
    let mut sources = SourceMap::new();
    let parsed = parse(&mut sources, "m.pir", text);
    assert!(
        !parsed.has_errors(),
        "fixture does not parse: {:?}",
        parsed.diagnostics.codes()
    );
    parsed.document
}

fn lower_expr(formula: &str) -> IrExpr {
    let doc = document(&format!(
        r#"
format = "pir/1"
module = "m"

[[component]]
name = "x"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "{formula}"
"#
    ));
    let component = doc.component("x").unwrap();
    predictable_check::lower::expr(&doc.arena, component.expr.unwrap()).expect("lowers")
}

#[test]
fn every_aggregate_spelling_becomes_an_agg_node() {
    for (formula, op) in [
        ("sum(flow)", AggOp::Sum),
        ("sum_kahan(flow)", AggOp::SumKahan),
        ("npv(flow, disc)", AggOp::Npv),
        ("first(flow)", AggOp::First),
        ("last(flow)", AggOp::Last),
        ("max_over(flow)", AggOp::MaxOver),
        ("min_over(flow)", AggOp::MinOver),
        ("count_while(in_term)", AggOp::CountWhile),
    ] {
        match lower_expr(formula) {
            IrExpr::Agg { op: got, .. } => assert_eq!(got, op, "{formula}"),
            other => panic!("{formula} lowered to {other:?}"),
        }
    }
}

#[test]
fn npv_carries_its_discount_series_in_the_second_operand() {
    match lower_expr("npv(flow, disc)") {
        IrExpr::Agg { op, value, pred } => {
            assert_eq!(op, AggOp::Npv);
            assert_eq!(*value, IrExpr::r#ref("flow"));
            assert_eq!(pred.map(|p| *p), Some(IrExpr::r#ref("disc")));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_predicated_sum_keeps_its_predicate() {
    match lower_expr("sum(flow, in_term)") {
        IrExpr::Agg { pred: Some(p), .. } => assert_eq!(*p, IrExpr::r#ref("in_term")),
        other => panic!("{other:?}"),
    }
}

#[test]
fn at_and_the_index_form_lower_to_the_same_node() {
    // §2.8: `at(x, k)` and `x[k]` are the same operation.
    assert_eq!(
        lower_expr("at(flow, 5)"),
        IrExpr::At {
            name: "flow".into(),
            k: 5
        }
    );
    assert_eq!(lower_expr("flow[5]"), lower_expr("at(flow, 5)"));
}

#[test]
fn a_non_aggregate_call_stays_a_call() {
    match lower_expr("to_monthly(rate)") {
        IrExpr::Call { func, args } => {
            assert_eq!(func, "to_monthly");
            assert_eq!(args.len(), 1);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn lowering_an_aggregate_makes_the_component_stage_two() {
    // The IR computes `stage` from the expression, so the rewrite is what
    // decides it: leave `npv` as a call and the component is stage 1, which is
    // wrong (§2.2).
    let doc = document(
        r#"
format = "pir/1"
module = "m"

[[component]]
name = "flow"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "1.0"

[[component]]
name = "pv"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "npv(flow, disc)"
"#,
    );
    let module = predictable_check::lower::module(&doc);
    assert_eq!(module.component("flow").unwrap().stage(), Stage::One);
    assert_eq!(module.component("pv").unwrap().stage(), Stage::Two);
}

#[test]
fn a_lowered_module_round_trips_through_pir_json() {
    // The IR's own encoding is the check that lowering produced a well-formed
    // module and not just a structurally similar one.
    let doc = document(
        r#"
format = "pir/1"
module = "term"

[timeline]
basis = "annual"
periods = 40
origin = "policy"
valuation_date = 2026-06-30
year_convention = "act/365"

[[enum]]
name = "Gender"
values = ["M", "F"]

[[modelpoint_field]]
name = "policy_number"
dtype = "str"
required = true
key = true

[[modelpoint_field]]
name = "sum_assured"
dtype = "f64"
unit = "money"
required = true

[[assumption]]
name = "valuation_rate"
dtype = "f64"
unit = "rate(annual)"
shape = "Scalar"

[[table]]
name = "lapse_rates"
keys = [{ name = "policy_year", dtype = "i64", policy = "step" }]
values = [{ name = "lapse_pa", dtype = "f64", unit = "prob" }]
on_missing = "default(0.0)"
source = "inline"
digest = "sha256:31aa"
rows = [
  [1, 0.12],
  [2, 0.08],
]

[[component]]
name = "flow"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
init = "sum_assured"
expr = "flow[t-1] * (1 + valuation_rate) - lapse_rates@(policy_year)"

[[component]]
name = "pv"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "npv(flow, disc)"
"#,
    );
    let module = predictable_check::lower::module(&doc);
    assert_eq!(module.timeline.as_ref().unwrap().periods, 40);
    assert_eq!(module.tables[0].arity(), 1);
    assert!(module.tables[0].has_presence_bit());
    assert_eq!(module.enums[0].code_of("F"), Some(1));

    let file = predictable_ir::PirFile::from(module.clone());
    let json = file.to_json_pretty().unwrap();
    assert_eq!(predictable_ir::PirFile::from_json(&json).unwrap(), file);
}

#[test]
fn a_broken_expression_has_no_ir_form() {
    // Inventing an IR node for a tree that did not parse would hide the
    // diagnostic that produced it.
    let mut sources = SourceMap::new();
    let parsed = parse(
        &mut sources,
        "m.pir",
        r#"
format = "pir/1"
module = "m"

[[component]]
name = "x"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "flow[t+1]"
"#,
    );
    let component = parsed.document.component("x").unwrap();
    assert!(
        predictable_check::lower::expr(&parsed.document.arena, component.expr.unwrap()).is_none()
    );
}
