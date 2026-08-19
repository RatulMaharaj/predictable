//! Behavioural tests for the planner, written against `03-engine.md` §3–§4.3
//! rather than against the implementation: each one states a property the spec
//! promises and checks the plan keeps it.

use predictable_check::Input;
use predictable_ir::{Kind, Stage};
use predictable_plan::{
    plan, plan_sources, FullReason, OptLevel, PlanOptions, Retention, TIMELINE_MODULE,
};

const MODEL: &str = include_str!("model.pir");

fn planned(opt: OptLevel) -> predictable_plan::Plan {
    let options = PlanOptions {
        opt,
        program_digest: "sha256:test".to_string(),
        ..PlanOptions::default()
    };
    plan_sources(&[Input::new("model.pir", MODEL)], &options).expect("model plans")
}

// ---------------------------------------------------------------------------
// §3.1 slot allocation
// ---------------------------------------------------------------------------

#[test]
fn every_value_the_runtime_can_name_gets_a_slot() {
    let plan = planned(OptLevel::O1);

    // 2 assumptions + 1 derived scalar.
    assert_eq!(plan.scalars.len(), 3);
    // 2 modelpoint fields + 3 stage-2 PerMP components.
    assert_eq!(plan.permp.len(), 5);
    // 8 timeline fields + 5 series components.
    assert_eq!(plan.series.len(), 13);

    // Slot ids are globally dense: every id in 0..n appears exactly once.
    let total = plan.scalars.len() + plan.permp.len() + plan.series.len();
    assert_eq!(plan.slot_refs.len(), total);
    let mut seen: Vec<u32> = plan
        .scalars
        .iter()
        .map(|s| s.info.id.0)
        .chain(plan.permp.iter().map(|s| s.info.id.0))
        .chain(plan.series.iter().map(|s| s.info.id.0))
        .collect();
    seen.sort_unstable();
    assert_eq!(seen, (0..total as u32).collect::<Vec<_>>());
}

#[test]
fn timeline_fields_are_series_slots_in_their_own_module() {
    let plan = planned(OptLevel::O1);
    let t = plan.series_named("t").expect("`t` is a slot");
    assert_eq!(t.info.module_path, TIMELINE_MODULE);
    assert_eq!(t.info.kind, Kind::InputTimeline);
    assert_eq!(t.info.qualified_id(), "<timeline>.t");
    // All eight of `01-ir.md` §5, and the timeline sorts first.
    assert_eq!(
        plan.series
            .iter()
            .filter(|s| s.info.module_path == TIMELINE_MODULE)
            .count(),
        8
    );
}

#[test]
fn inputs_have_slots_but_no_expression() {
    let plan = planned(OptLevel::O1);
    let sum_assured = plan.permp_named("sum_assured").unwrap();
    assert_eq!(sum_assured.info.kind, Kind::InputModelpoint);
    assert!(sum_assured.expr.is_none());
    assert!(plan.scalar_named("valuation_rate").unwrap().expr.is_none());
    assert!(plan.scalar_named("annual_rate").unwrap().expr.is_some());
}

#[test]
fn outputs_are_the_emission_set_in_declaration_order() {
    let plan = planned(OptLevel::O1);
    let names: Vec<String> = plan
        .outputs
        .iter()
        .map(|id| plan.info(*id).name.clone())
        .collect();
    assert_eq!(
        names,
        [
            "claims",
            "reserve_seed",
            "pv_claims",
            "total_claims",
            "margin"
        ]
    );
}

// ---------------------------------------------------------------------------
// §3.2 ordering
// ---------------------------------------------------------------------------

#[test]
fn the_order_is_total_and_respects_lag_zero_dependence() {
    let plan = planned(OptLevel::O1);
    let order = plan.order();
    let mut sorted = order.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), order.len(), "every slot emitted exactly once");
    assert_eq!(sorted.len(), plan.slot_refs.len());

    let position = |name: &str| {
        plan.order()
            .iter()
            .position(|id| plan.info(*id).name == name)
            .unwrap_or_else(|| panic!("{name} is not in the order"))
    };
    // `claims = num_pols_if * q_x * sum_assured`: both reads are lag 0.
    assert!(position("num_pols_if") < position("claims"));
    assert!(position("q_x") < position("claims"));
    // `margin` reads `pv_claims`, another stage-2 value.
    assert!(position("pv_claims") < position("margin"));
}

#[test]
fn a_lagged_self_reference_is_not_an_ordering_constraint() {
    // `num_pols_if[t] = num_pols_if[t-1] * (1 - q_x[t-1])` is a legal self-loop
    // (`01-ir.md` §3.1): last period is already data. It must plan, and it must
    // not appear twice.
    let plan = planned(OptLevel::O1);
    assert_eq!(
        plan.stage1
            .iter()
            .filter(|id| plan.info(**id).name == "num_pols_if")
            .count(),
        1
    );
    assert_eq!(plan.series_named("num_pols_if").unwrap().max_lag, 1);
}

#[test]
fn the_ready_set_breaks_ties_on_module_path_then_declaration_index() {
    // Two modules, four mutually independent components. Nothing about the
    // formulas orders them, so the §3.2 min-heap decides: module `a` before
    // module `b`, and within a module, declaration order.
    let head = |module: &str| {
        format!(
            r#"
format = "pir/1"
module = "{module}"

[[component]]
name = "{module}_second"
kind = "Derived"
dtype = "f64"
shape = "Scalar"
unit = "none"
expr = "2.0"

[[component]]
name = "{module}_first"
kind = "Derived"
dtype = "f64"
shape = "Scalar"
unit = "none"
expr = "1.0"
"#
        )
    };
    let timeline = r#"
format = "pir/1"
module = "z_timeline"

[timeline]
basis = "annual"
periods = 3
origin = "policy"
valuation_date = 2026-06-30
"#;

    // Feed the modules to the planner in the *wrong* order on purpose.
    let inputs = [
        Input::new("b.pir", head("b")),
        Input::new("a.pir", head("a")),
        Input::new("z.pir", timeline),
    ];
    let plan = plan_sources(&inputs, &PlanOptions::default()).expect("plans");
    let names: Vec<&str> = plan
        .prologue
        .iter()
        .map(|id| plan.info(*id).name.as_str())
        .collect();
    assert_eq!(names, ["a_second", "a_first", "b_second", "b_first"]);
}

#[test]
fn the_order_is_a_pure_function_of_the_model() {
    // Same model, planned twice, in two processes' worth of allocation
    // patterns: identical order and identical digest. This is the property
    // `03-engine.md` §7 rests on.
    let a = planned(OptLevel::O1);
    let b = planned(OptLevel::O1);
    assert_eq!(a.order(), b.order());
    assert_eq!(a.order_digest, b.order_digest);
    assert_eq!(a.digest, b.digest);
    assert_eq!(a, b);
}

#[test]
fn order_digest_is_a_golden_value() {
    // A change in topological order is a diffable, reviewable event (§3.2). If
    // this assertion fails, the order changed — decide whether that was
    // intended before updating the constant.
    let plan = planned(OptLevel::O1);
    assert_eq!(
        plan.order_names(),
        [
            "sum_assured",
            "entry_age",
            "valuation_rate",
            "mortality_loading",
            "annual_rate",
            "t",
            "period_start_date",
            "period_end_date",
            "year_frac",
            "month_of_year",
            "policy_year",
            "policy_month",
            "is_anniversary",
            "discount_factor",
            "q_x",
            "num_pols_if",
            "claims",
            "reserve_seed",
            "pv_claims",
            "total_claims",
            "margin",
        ]
    );
    assert_eq!(plan.order_digest.len(), 64);
}

#[test]
fn moving_work_between_tapes_changes_the_order_digest() {
    // `--O0` leaves `discount_factor` in the per-`t` body instead of hoisting
    // it. The slot sequence is the same set; the digest must still differ,
    // because *where* the work happens changed.
    let o1 = planned(OptLevel::O1);
    let o0 = planned(OptLevel::O0);
    assert!(!o1.hoisted.is_empty());
    assert!(o0.hoisted.is_empty());
    assert_ne!(o1.order_digest, o0.order_digest);
    assert_ne!(o1.digest, o0.digest);
}

#[test]
fn the_plan_digest_moves_with_the_program_and_the_run_config() {
    let base = PlanOptions {
        program_digest: "sha256:aaa".to_string(),
        ..PlanOptions::default()
    };
    let other_program = PlanOptions {
        program_digest: "sha256:bbb".to_string(),
        ..base.clone()
    };
    let retain_all = PlanOptions {
        retain_all: true,
        ..base.clone()
    };
    let inputs = [Input::new("model.pir", MODEL)];
    let a = plan_sources(&inputs, &base).unwrap();
    let b = plan_sources(&inputs, &other_program).unwrap();
    let c = plan_sources(&inputs, &retain_all).unwrap();
    assert_ne!(a.digest, b.digest, "program digest is folded in");
    assert_ne!(a.digest, c.digest, "run config is folded in");
    // The order itself did not change, only the plan around it.
    assert_eq!(a.order_digest, b.order_digest);
}

#[test]
fn an_unchecked_cycle_is_an_error_not_a_panic() {
    use predictable_ir::{BinaryOp, Component, DType, Expr, Module, Shape};
    let mut module = Module::new("m");
    module.timeline = Some(predictable_ir::Timeline {
        basis: predictable_ir::Basis::Annual,
        periods: 3,
        origin: predictable_ir::Origin::Policy,
        valuation_date: "2026-06-30".to_string(),
        year_convention: "act/365".to_string(),
    });
    for (name, other) in [("a", "b"), ("b", "a")] {
        module.components.push(Component::derived(
            name,
            DType::F64,
            Shape::Scalar,
            Expr::binary(BinaryOp::Add, Expr::r#ref(other), Expr::f64(1.0)),
        ));
    }
    let err = plan(&[module], &PlanOptions::default()).expect_err("cycle refused");
    assert!(err.to_string().contains("E0201"), "{err}");
}

#[test]
fn planning_an_unchecked_model_is_refused() {
    let broken = r#"
format = "pir/1"
module = "m"

[timeline]
basis = "annual"
periods = 3
origin = "policy"
valuation_date = 2026-06-30

[[component]]
name = "a"
kind = "Derived"
dtype = "f64"
shape = "Scalar"
unit = "none"
expr = "nope + 1.0"
"#;
    let err =
        plan_sources(&[Input::new("m.pir", broken)], &PlanOptions::default()).expect_err("refused");
    match err {
        predictable_plan::PlanError::NotChecked(d) => assert_eq!(d[0].code, "E0203"),
        other => panic!("wrong error: {other}"),
    }
}

// ---------------------------------------------------------------------------
// §4.3 retention
// ---------------------------------------------------------------------------

#[test]
fn retention_defaults_to_a_power_of_two_ring() {
    let plan = planned(OptLevel::O1);
    // `num_pols_if` is read at lag 1 and nothing else: two periods kept.
    let n = plan.series_named("num_pols_if").unwrap();
    assert_eq!(n.max_lag, 1);
    assert_eq!(n.retention, Retention::Ring { len: 2 });
    assert_eq!(n.full_reason, None);
    // Ring lengths are powers of two so indexing is `t & (len - 1)`.
    for s in &plan.series {
        if let Retention::Ring { len } = s.retention {
            assert!(len.is_power_of_two(), "{} ring {len}", s.info.name);
            assert!(len > s.max_lag);
        }
    }
}

#[test]
fn a_never_lagged_intermediate_keeps_one_period() {
    let plan = planned(OptLevel::O1);
    let t = plan.series_named("period_start_date").unwrap();
    assert_eq!(t.max_lag, 0);
    assert_eq!(t.retention, Retention::Ring { len: 1 });
}

#[test]
fn outputs_reduce_targets_and_at_targets_are_retained_whole() {
    let plan = planned(OptLevel::O1);

    let claims = plan.series_named("claims").unwrap();
    assert_eq!(claims.retention, Retention::Full);
    assert_eq!(claims.full_reason, Some(FullReason::Output));

    // `q_x` is neither an output nor reduced — it is `q_x[0]` in `reserve_seed`.
    let q_x = plan.series_named("q_x").unwrap();
    assert_eq!(q_x.retention, Retention::Full);
    assert_eq!(q_x.full_reason, Some(FullReason::AtTarget));

    // `discount_factor` is the second argument of an `npv`: a reduce target.
    let options = PlanOptions {
        opt: OptLevel::O0,
        ..PlanOptions::default()
    };
    let unhoisted = plan_sources(&[Input::new("model.pir", MODEL)], &options).unwrap();
    let df = unhoisted.series_named("discount_factor").unwrap();
    assert_eq!(df.retention, Retention::Full);
    assert_eq!(df.full_reason, Some(FullReason::Reduced));
}

#[test]
fn retain_all_forces_full_and_costs_what_it_says_it_costs() {
    let plan = planned(OptLevel::O1);
    let options = PlanOptions {
        retain_all: true,
        ..PlanOptions::default()
    };
    let all = plan_sources(&[Input::new("model.pir", MODEL)], &options).unwrap();
    assert!(all.series.iter().all(|s| s.retention.is_full()));
    assert!(all
        .series
        .iter()
        .all(|s| s.full_reason == Some(FullReason::RetainAll)));
    // The projected footprint is what the CLI prints before allocating.
    assert!(
        all.series_bytes_per_chunk(1024) > plan.series_bytes_per_chunk(1024),
        "retaining everything must cost more"
    );
}

// ---------------------------------------------------------------------------
// §3.4 hoisting
// ---------------------------------------------------------------------------

#[test]
fn a_series_free_of_modelpoint_dependence_leaves_the_loop() {
    let plan = planned(OptLevel::O1);
    // Timeline fields are invariant by construction.
    assert!(plan.series_named("t").unwrap().hoistable);
    // `discount_factor` reads `t`, itself at lag 1, and a scalar.
    assert!(plan.series_named("discount_factor").unwrap().hoistable);
    // `q_x` reads `entry_age`, a modelpoint field.
    assert!(!plan.series_named("q_x").unwrap().hoistable);
    // `num_pols_if` reads `q_x`, transitively modelpoint-dependent.
    assert!(!plan.series_named("num_pols_if").unwrap().hoistable);
    assert!(!plan.series_named("claims").unwrap().hoistable);

    let hoisted: Vec<&str> = plan
        .hoisted
        .iter()
        .map(|id| plan.info(*id).name.as_str())
        .collect();
    assert!(hoisted.contains(&"discount_factor"));
    assert!(!hoisted.contains(&"q_x"));
    // Hoisted slots leave the per-`t` body entirely.
    assert!(plan
        .stage1
        .iter()
        .all(|id| plan.info(*id).name != "discount_factor"));
}

#[test]
fn dependence_through_a_lookup_key_defeats_hoisting() {
    let source = r#"
format = "pir/1"
module = "m"

[timeline]
basis = "annual"
periods = 5
origin = "policy"
valuation_date = 2026-06-30

[[modelpoint_field]]
name = "entry_age"
dtype = "i64"
unit = "years"
required = true

[[table]]
name = "mort"
source = "inline"
keys = [{ name = "age", dtype = "i64", policy = "exact" }]
values = [{ name = "qx", dtype = "f64", unit = "prob" }]
rows = [[30, 0.001], [31, 0.0012]]

[[component]]
name = "by_time"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "prob"
timing = "start"
expr = "mort@(30 + t)"

[[component]]
name = "by_policy"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "prob"
timing = "start"
expr = "mort@(entry_age + t)"
"#;
    let plan = plan_sources(&[Input::new("m.pir", source)], &PlanOptions::default()).unwrap();
    // A table is run-constant, so a lookup on a time-only key hoists...
    assert!(plan.series_named("by_time").unwrap().hoistable);
    // ...and one keyed on a modelpoint field does not.
    assert!(!plan.series_named("by_policy").unwrap().hoistable);
}

#[test]
fn o0_hoists_nothing() {
    let plan = planned(OptLevel::O0);
    assert!(plan.hoisted.is_empty());
    assert!(plan.series.iter().all(|s| !s.hoistable));
    // Every series is back in the per-`t` body, and the body is still ordered.
    assert_eq!(plan.stage1.len(), plan.series.len());
}

// ---------------------------------------------------------------------------
// stages
// ---------------------------------------------------------------------------

#[test]
fn aggregates_land_in_stage_two_and_nothing_else_does() {
    let plan = planned(OptLevel::O1);
    let stage2: Vec<&str> = plan
        .stage2
        .iter()
        .map(|id| plan.info(*id).name.as_str())
        .collect();
    assert_eq!(stage2, ["pv_claims", "total_claims", "margin"]);
    for id in &plan.stage2 {
        // `margin` has no `Agg` of its own; it is stage 2 by *ordering*, and the
        // planner records the IR's computed stage verbatim (§2.2).
        let info = plan.info(*id);
        if info.name != "margin" {
            assert_eq!(info.stage, Stage::Two);
        }
    }
    assert!(plan
        .prologue
        .iter()
        .all(|id| plan.info(*id).stage == Stage::One));
}

#[test]
fn the_plan_round_trips_through_json() {
    // The plan is data: the viz server (T28) reads the ordering from here and
    // never recomputes it, so it has to serialise.
    let plan = planned(OptLevel::O1);
    let json = serde_json::to_string(&plan).unwrap();
    let back: predictable_plan::Plan = serde_json::from_str(&json).unwrap();
    assert_eq!(plan, back);
}
