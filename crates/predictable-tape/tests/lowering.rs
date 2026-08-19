//! Behavioural tests for tape lowering.
//!
//! Every test here goes through the real pipeline — parse, check, plan, lower —
//! and asserts on the ops that come out, not on internal calls. The tape is the
//! engine's contract with the kernel, so what is worth pinning is exactly what
//! the kernel will see.

use predictable_check::Input;
use predictable_ir::{AggOp, Timing};
use predictable_plan::{plan_sources, PlanOptions};
use predictable_tape::{lower_plan, LowerError, Op, Tape, TapeProgram, TimeField};

const HEAD: &str = r#"
format = "pir/1"
module = "m"

[timeline]
basis = "annual"
periods = 10
origin = "policy"
valuation_date = 2026-06-30

[[modelpoint_field]]
name = "sum_assured"
dtype = "f64"
unit = "none"
required = true

[[modelpoint_field]]
name = "rate"
dtype = "f64"
unit = "none"
required = true
"#;

fn program(body: &str) -> TapeProgram {
    let src = format!("{HEAD}{body}");
    let plan = plan_sources(&[Input::new("m.pir", &src)], &PlanOptions::default())
        .unwrap_or_else(|e| panic!("plan failed: {e}"));
    lower_plan(&plan).unwrap_or_else(|e| panic!("lower failed: {e}"))
}

fn try_program(body: &str) -> Result<TapeProgram, LowerError> {
    let src = format!("{HEAD}{body}");
    let plan = plan_sources(&[Input::new("m.pir", &src)], &PlanOptions::default())
        .unwrap_or_else(|e| panic!("plan failed: {e}"));
    lower_plan(&plan)
}

fn series(name: &str, extra: &str, expr: &str) -> String {
    format!(
        r#"
[[component]]
name = "{name}"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "none"
timing = "end"
{extra}
expr = "{expr}"
"#
    )
}

fn ops(t: &Tape) -> Vec<Op> {
    t.ops.clone()
}

// ---------------------------------------------------------------------------
// The op set
// ---------------------------------------------------------------------------

#[test]
fn arithmetic_lowers_to_a_straight_line_with_one_store() {
    let p = program(&series("x", "", "sum_assured * 2.0 + 1.0"));
    let body = ops(&p.stage1.body);
    assert!(matches!(body[0], Op::LoadPerMp(..)));
    assert!(matches!(body.last().unwrap(), Op::StoreCur(..)));
    // No control flow ops exist at all: a tape is one basic block by
    // construction (03-engine.md §3.3).
    assert!(body.iter().all(|o| !matches!(o, Op::Select(..))));
    assert_eq!(body.iter().filter(|o| matches!(o, Op::Add(..))).count(), 1);
    assert_eq!(body.iter().filter(|o| matches!(o, Op::Mul(..))).count(), 1);
}

#[test]
fn the_timeline_is_computed_not_loaded() {
    let p = program(&series("x", "", "sum_assured * t"));
    assert!(ops(&p.stage1.body)
        .iter()
        .any(|o| matches!(o, Op::LoadTime(_, TimeField::T))));
}

#[test]
fn a_loop_invariant_series_is_read_as_a_broadcast() {
    // `disc` depends only on a scalar assumption, so the planner hoists it;
    // reading it from the per-modelpoint loop must be a stride-0 load.
    let body = format!(
        r#"
[[assumption]]
name = "v"
dtype = "f64"
shape = "Scalar"
unit = "none"
{}{}"#,
        series("disc", r#"init = "1.0""#, "disc[t-1] * v"),
        series("x", "", "disc * sum_assured")
    );
    let p = program(&body);
    assert!(!p.hoisted.body.is_empty(), "disc should be hoisted");
    assert!(ops(&p.stage1.body)
        .iter()
        .any(|o| matches!(o, Op::LoadHoistedCur(..))));
    assert!(ops(&p.hoisted.body)
        .iter()
        .any(|o| matches!(o, Op::StoreHoisted(..))));
}

// ---------------------------------------------------------------------------
// Static trap masking (01-ir.md §2.6, Q7)
// ---------------------------------------------------------------------------

#[test]
fn a_division_outside_an_if_is_unmasked() {
    let p = program(&series("x", "", "sum_assured / rate"));
    let div = ops(&p.stage1.body)
        .into_iter()
        .find(|o| matches!(o, Op::Div { .. }))
        .expect("a div");
    assert_eq!(div.mask(), None);
}

#[test]
fn the_defensive_idiom_masks_the_untaken_arm() {
    // `if rate == 0 then 0 else 1/rate` — the most common defensive idiom in
    // actuarial code, and it must not trap on the zero lanes.
    let p = program(&series(
        "x",
        "",
        "if rate == 0.0 then 0.0 else sum_assured / rate",
    ));
    let body = ops(&p.stage1.body);
    let div = body
        .iter()
        .find(|o| matches!(o, Op::Div { .. }))
        .expect("a div");
    let mask = div.mask().expect("the else arm's div is masked");
    // The mask is the negation of the condition, materialised statically.
    let not = body
        .iter()
        .find(|o| matches!(o, Op::Not(..)))
        .expect("a not");
    assert_eq!(not.def(), Some(mask));
    // Both arms are still evaluated and joined by a select — no branch.
    assert!(body.iter().any(|o| matches!(o, Op::Select(..))));
}

#[test]
fn nested_ifs_conjoin_their_masks() {
    let p = program(&series(
        "x",
        "",
        "if rate > 0.0 then (if sum_assured > 0.0 then sum_assured / rate else 0.0) else 0.0",
    ));
    let body = ops(&p.stage1.body);
    let div = body
        .iter()
        .find(|o| matches!(o, Op::Div { .. }))
        .expect("a div");
    let mask = div.mask().expect("masked");
    // The mask of a doubly-nested arm is an `And` of the two conditions.
    let and = body
        .iter()
        .find(|o| matches!(o, Op::And(..)))
        .expect("an and");
    assert_eq!(and.def(), Some(mask));
}

#[test]
fn an_arm_without_a_trap_gets_no_mask_ops() {
    let p = program(&series("x", "", "if rate > 0.0 then sum_assured else 0.0"));
    let body = ops(&p.stage1.body);
    assert!(
        !body.iter().any(|o| matches!(o, Op::Not(..) | Op::And(..))),
        "no mask should be materialised for arms that cannot trap"
    );
}

#[test]
fn a_lookup_in_an_arm_is_masked_too() {
    let body = format!(
        r#"
[[table]]
name = "qx"
keys = [{{ name = "age", dtype = "i64", policy = "clamp" }}]
values = [{{ name = "q", dtype = "f64", unit = "prob" }}]
on_missing = "error"
source = "tables/qx.csv"
digest = "sha256:00"
{}"#,
        series("x", "", "if rate > 0.0 then qx@(policy_year) else 0.0")
    );
    let p = program(&body);
    let lookup = ops(&p.stage1.body)
        .into_iter()
        .find(|o| matches!(o, Op::Lookup { .. }))
        .expect("a lookup");
    assert!(lookup.mask().is_some());
    assert_eq!(p.tables, vec!["qx".to_string()]);
}

#[test]
fn every_trapping_op_carries_a_site() {
    let p = program(&series("x", "", "sum_assured / rate"));
    let div = ops(&p.stage1.body).into_iter().find(|o| o.traps()).unwrap();
    let site = &p.stage1.body.sites[div.site().unwrap().0 as usize];
    assert_eq!(site.path, "expr", "the trap names the expression it is in");
    assert!(!site.pre_origin_default);
}

// ---------------------------------------------------------------------------
// Loop peeling (03-engine.md §5.2)
// ---------------------------------------------------------------------------

#[test]
fn init_replaces_the_formula_at_the_origin() {
    let p = program(&series("x", r#"init = "1.0""#, "x[t-1] + sum_assured"));
    let t0 = ops(&p.stage1.t0);
    // The seed is evaluated and stored first, then read back as x[0].
    assert!(matches!(t0[0], Op::ConstF(..)));
    assert!(matches!(t0[1], Op::StoreSeed(..)));
    assert!(matches!(t0[2], Op::LoadSeed(..)));
    assert!(matches!(t0[3], Op::StoreCur(..)));
    // The t = 0 tape contains no lag load at all.
    assert!(!t0.iter().any(|o| matches!(o, Op::LoadLag(..))));
    // The body does.
    assert!(ops(&p.stage1.body)
        .iter()
        .any(|o| matches!(o, Op::LoadLag(_, _, 1))));
}

#[test]
fn a_deep_lag_peels_one_prefix_tape_per_period() {
    let p = program(&series(
        "x",
        r#"init = "1.0""#,
        "x[t-1] + x[t-3] + sum_assured",
    ));
    assert_eq!(p.peel, 3, "max_lag_global");
    assert_eq!(p.stage1.prefix.len(), 2, "t = 1 and t = 2 are peeled");

    // t = 1: lag 1 is in range, lag 3 is not.
    let t1 = ops(p.stage1.at(1));
    assert!(t1.iter().any(|o| matches!(o, Op::LoadLag(_, _, 1))));
    assert!(!t1.iter().any(|o| matches!(o, Op::LoadLag(_, _, 3))));
    assert!(t1.iter().any(|o| matches!(o, Op::LoadSeed(..))));

    // t = 3 onwards is the body, where both lags are ordinary loads.
    let body = ops(p.stage1.at(3));
    assert!(body.iter().any(|o| matches!(o, Op::LoadLag(_, _, 3))));
    assert!(!body.iter().any(|o| matches!(o, Op::LoadSeed(..))));
    assert!(std::ptr::eq(p.stage1.at(3), p.stage1.at(9)));
}

#[test]
fn a_pre_origin_lag_without_init_is_the_dtype_zero() {
    let p = program(&series("x", "", "x[t-1] + sum_assured"));
    let t0 = ops(&p.stage1.t0);
    assert!(!t0.iter().any(|o| matches!(o, Op::LoadSeed(..))));
    assert!(matches!(t0[0], Op::ConstF(..)));
    assert_eq!(p.stage1.t0.const_f[0], 0.0);
    // The provenance note §3.3 asks for is recorded against the lag's path.
    let site = p
        .stage1
        .t0
        .sites
        .iter()
        .find(|s| s.pre_origin_default)
        .expect("a pre_origin_default site");
    assert_eq!(site.path, "expr.lhs");
}

// ---------------------------------------------------------------------------
// Stage 2
// ---------------------------------------------------------------------------

#[test]
fn npv_reads_two_whole_series_and_the_value_timing() {
    let body = format!(
        r#"
[[assumption]]
name = "v"
dtype = "f64"
shape = "Scalar"
unit = "none"
{}{}
[[component]]
name = "pv"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "none"
expr = "npv(cf, disc)"
"#,
        series("disc", r#"init = "1.0""#, "disc[t-1] * v"),
        series("cf", "", "sum_assured * rate")
    );
    let p = program(&body);
    let npv = ops(&p.stage2)
        .into_iter()
        .find(|o| matches!(o, Op::Npv { .. }))
        .expect("an npv");
    let Op::Npv { timing, .. } = npv else {
        unreachable!()
    };
    assert_eq!(timing, Timing::End, "cf is timing = end");
    assert!(ops(&p.stage2)
        .iter()
        .any(|o| matches!(o, Op::StorePerMp(..))));
    assert!(p.stage1.body.is_empty() || !p.stage1.body.is_empty());
}

#[test]
fn a_reduction_lowers_to_reduce_over_the_slot() {
    let body = format!(
        r#"{}
[[component]]
name = "total"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "none"
expr = "sum(cf)"
"#,
        series("cf", "", "sum_assured * rate")
    );
    let p = program(&body);
    let red = ops(&p.stage2)
        .into_iter()
        .find(|o| matches!(o, Op::Reduce { .. }))
        .expect("a reduce");
    let Op::Reduce { agg, pred, .. } = red else {
        unreachable!()
    };
    assert_eq!(agg, AggOp::Sum);
    assert_eq!(pred, None);
}

#[test]
fn a_computed_aggregate_operand_is_refused_rather_than_guessed() {
    let body = format!(
        r#"{}
[[component]]
name = "total"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "none"
expr = "sum(cf * 2.0)"
"#,
        series("cf", "", "sum_assured * rate")
    );
    match try_program(&body) {
        Err(LowerError::AggArgumentNotASeries { path, .. }) => assert_eq!(path, "expr.value"),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Registers, CSE, digests
// ---------------------------------------------------------------------------

#[test]
fn registers_are_reused_and_bounded_by_n_regs() {
    let p = program(&series(
        "x",
        "",
        "sum_assured * rate + sum_assured * rate + sum_assured * rate + sum_assured",
    ));
    let tape = &p.stage1.body;
    assert!(tape.n_regs <= 4, "n_regs was {}", tape.n_regs);
    for op in &tape.ops {
        op.for_each_reg(|r, _| assert!(r.0 < tape.n_regs, "{r} out of frame"));
    }
}

#[test]
fn identical_subexpressions_share_a_register_within_a_component() {
    let with_cse = program(&series(
        "x",
        "",
        "(sum_assured + rate) * (sum_assured + rate)",
    ));
    let without = program(&series(
        "x",
        "",
        "(sum_assured + rate) * (sum_assured - rate)",
    ));
    assert!(
        with_cse.stage1.body.len() < without.stage1.body.len(),
        "the repeated subtree should be computed once"
    );
    assert_eq!(
        with_cse
            .stage1
            .body
            .ops
            .iter()
            .filter(|o| matches!(o, Op::Add(..)))
            .count(),
        1
    );
}

#[test]
fn cse_never_merges_two_divisions() {
    // Merging them would merge their trap sites, and a trap must name the
    // expression the author wrote (§3.5).
    let p = program(&series("x", "", "sum_assured / rate + sum_assured / rate"));
    assert_eq!(
        p.stage1
            .body
            .ops
            .iter()
            .filter(|o| matches!(o, Op::Div { .. }))
            .count(),
        2
    );
}

#[test]
fn cse_does_not_cross_component_boundaries() {
    let body = format!(
        "{}{}",
        series("a", "", "sum_assured + rate"),
        series("b", "", "sum_assured + rate")
    );
    let p = program(&body);
    assert_eq!(
        p.stage1
            .body
            .ops
            .iter()
            .filter(|o| matches!(o, Op::Add(..)))
            .count(),
        2,
        "explain() must be able to show each component's own arithmetic"
    );
}

#[test]
fn constants_are_interned_by_bit_pattern() {
    let p = program(&series("x", "", "sum_assured * 2.0 + 2.0 - -0.0"));
    let pool = &p.stage1.body.const_f;
    assert_eq!(pool.iter().filter(|x| **x == 2.0).count(), 1);
    // 0.0 and -0.0 are equal but not identical: interning by value would
    // change bits.
    assert!(pool.iter().any(|x| x.is_sign_negative()));
}

#[test]
fn lowering_is_deterministic_and_digested() {
    let body = series("x", r#"init = "1.0""#, "x[t-1] + sum_assured / rate");
    let a = program(&body);
    let b = program(&body);
    assert_eq!(a.digest, b.digest);
    assert_eq!(a.text(), b.text());
    assert_eq!(a.digest.len(), 64);

    let changed = program(&series(
        "x",
        r#"init = "2.0""#,
        "x[t-1] + sum_assured / rate",
    ));
    assert_ne!(a.digest, changed.digest, "a lowering change is diffable");
}

#[test]
fn a_tape_program_round_trips_through_serde() {
    let p = program(&series(
        "x",
        r#"init = "1.0""#,
        "x[t-1] + sum_assured / rate",
    ));
    let json = serde_json::to_string(&p).unwrap();
    let back: TapeProgram = serde_json::from_str(&json).unwrap();
    assert_eq!(p, back);
}

#[test]
fn every_slot_has_a_tape_range() {
    let p = program(&format!(
        "{}{}",
        series("a", "", "sum_assured + rate"),
        series("b", "", "a * 2.0")
    ));
    let tape = &p.stage1.body;
    assert_eq!(tape.ranges.len(), 2);
    for r in &tape.ranges {
        assert!(!r.is_empty());
        assert!(matches!(tape.ops[r.end as usize - 1], Op::StoreCur(..)));
    }
}

// ---------------------------------------------------------------------------
// The timing builtins (01-ir.md §2.5) are re-expressed, not given ops
// ---------------------------------------------------------------------------

#[test]
fn shift_is_exactly_the_lag_it_is_sugar_for() {
    let shifted = program(&format!(
        "{}{}",
        series("flow", "", "sum_assured * rate"),
        series("x", "", "shift(flow, 1)")
    ));
    let lagged = program(&format!(
        "{}{}",
        series("flow", "", "sum_assured * rate"),
        series("x", "", "flow[t-1]")
    ));
    assert_eq!(shifted.stage1.body.text(), lagged.stage1.body.text());
}

#[test]
fn diff_becomes_a_subtraction_of_two_loads() {
    let p = program(&format!(
        "{}{}",
        series("flow", "", "sum_assured * rate"),
        series("x", "", "diff(flow)")
    ));
    let body = ops(&p.stage1.body);
    let range = p.stage1.body.ranges.last().unwrap();
    let tail = &body[range.start as usize..range.end as usize];
    assert!(matches!(tail[0], Op::LoadCur(..)));
    assert!(matches!(tail[1], Op::LoadLag(_, _, 1)));
    assert!(matches!(tail[2], Op::Sub(..)));
}

#[test]
fn cum_gets_an_accumulator_the_kernel_carries_across_t() {
    let p = program(&format!(
        "{}{}",
        series("flow", "", "sum_assured * rate"),
        series("x", "", "cum(flow)")
    ));
    assert_eq!(p.stage1.body.accumulators, 1);
    assert!(ops(&p.stage1.body).iter().any(|o| matches!(o, Op::Cum(..))));
}

#[test]
fn a_repeated_load_is_issued_once_even_across_if_arms() {
    // `rate` appears in the condition and in the else arm. A load cannot trap,
    // so the mask it was first evaluated under is irrelevant and one register
    // serves both.
    let p = program(&series(
        "x",
        "",
        "if rate == 0.0 then 0.0 else sum_assured / rate",
    ));
    assert_eq!(
        p.stage1
            .body
            .ops
            .iter()
            .filter(|o| matches!(o, Op::LoadPerMp(..)))
            .count(),
        2,
        "one load for `rate`, one for `sum_assured`"
    );
}
