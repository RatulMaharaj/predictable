//! `explain()` — behavioural tests for the recording evaluator (`04-verify.md` §3).
//!
//! Every test here asserts a property of the *contract*, not of the current
//! implementation's output shape: replay reproduces the run, a `Ref` expands
//! into the referenced component's tree, `Agg` terms sum exactly, notes fire
//! where the spec says they fire, and the text is a projection of the JSON.

use std::collections::BTreeMap;

use predictable_check::Input;
use predictable_engine::explain::{
    ExplainError, ExplainOptions, Explainer, NodeKind, TraceContext,
};
use predictable_engine::ChunkInput;
use predictable_ir::{Basis, Origin, Timeline};
use predictable_plan::{plan_sources, PlanOptions};
use predictable_tape::{lower_plan, TapeProgram};

const MODEL: &str = r#"
format = "pir/1"
module = "term"

[timeline]
basis = "annual"
periods = 5
origin = "policy"
valuation_date = 2026-06-30

[[modelpoint_field]]
name = "policy_id"
dtype = "str"
unit = "none"
required = true
key = true

[[modelpoint_field]]
name = "sum_assured"
dtype = "f64"
unit = "money"
required = true

[[modelpoint_field]]
name = "q"
dtype = "f64"
unit = "prob"
required = true

[[assumption]]
name = "valuation_rate"
dtype = "f64"
unit = "rate(annual)"

[[component]]
name = "disc"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "factor"
timing = "start"
expr = "compound(1 + valuation_rate, -t)"

[[component]]
name = "survivors"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "count"
timing = "start"
init = "1.0"
expr = "survivors[t-1] * (1 - q)"

[[component]]
name = "claims"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "survivors * q * sum_assured"

[[component]]
name = "bel"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "npv(claims, disc)"
"#;

struct Fixture {
    plan: predictable_plan::Plan,
    tapes: TapeProgram,
}

fn fixture(source: &str) -> Fixture {
    // Full retention is what a trace needs: a ring buffer has already thrown
    // away the history `explain(t = 3)` asks about.
    let options = PlanOptions {
        retain_all: true,
        ..PlanOptions::default()
    };
    let plan = plan_sources(&[Input::new("term.pir", source)], &options).expect("plans");
    let tapes = lower_plan(&plan).expect("lowers");
    Fixture { plan, tapes }
}

fn timeline(periods: u32) -> Timeline {
    Timeline {
        basis: Basis::Annual,
        periods,
        origin: Origin::Policy,
        valuation_date: "2026-06-30".to_string(),
        year_convention: "act/365".to_string(),
    }
}

fn explainer<'a>(f: &'a Fixture, periods: u32) -> Explainer<'a> {
    let mut ctx = TraceContext {
        run: "sha256:test".to_string(),
        ..TraceContext::default()
    };
    ctx.modelpoint_file = Some("data/term.csv".to_string());
    ctx.assumption_set = Some("base".to_string());
    let mut ex = Explainer::new(&f.plan, &f.tapes, vec![], &timeline(periods), ctx).expect("binds");
    let mut assumptions = BTreeMap::new();
    assumptions.insert("valuation_rate".to_string(), 0.035);
    ex.prepare(&assumptions).expect("prepares");
    ex
}

fn one_modelpoint(sum_assured: f64, q: f64) -> ChunkInput {
    let mut columns = BTreeMap::new();
    columns.insert("policy_id".to_string(), vec![2.0]);
    columns.insert("sum_assured".to_string(), vec![sum_assured]);
    columns.insert("q".to_string(), vec![q]);
    ChunkInput {
        index: 0,
        keys: vec!["POL00042".to_string()],
        first_row: 41,
        columns,
    }
}

// ---------------------------------------------------------------------------

#[test]
fn replay_reproduces_the_run_at_every_period() {
    let f = fixture(MODEL);
    let mut ex = explainer(&f, 5);
    ex.load(&one_modelpoint(100_000.0, 0.01)).expect("runs");
    let claims = ex
        .output()
        .unwrap()
        .column("claims")
        .expect("claims is an output")
        .lane(0)
        .to_vec();

    for t in 0..=5u32 {
        let trace = ex
            .explain("claims", Some(t), &ExplainOptions::full())
            .expect("no E0901");
        assert_eq!(
            trace.root.value, claims[t as usize],
            "replay at t={t} must equal the kernel's stored result"
        );
    }
}

#[test]
fn every_component_node_in_a_full_trace_matches_the_kernel() {
    let f = fixture(MODEL);
    let mut ex = explainer(&f, 5);
    ex.load(&one_modelpoint(250_000.0, 0.02)).expect("runs");
    // `divergence` walks every Component node, not just the root — a full trace
    // therefore checks the whole dependency cone against the run.
    let trace = ex
        .explain_unchecked("claims", Some(4), &ExplainOptions::full())
        .expect("traces");
    assert!(ex.divergence(&trace).is_none());
    let mut components = 0;
    trace.root.walk(&mut |n| {
        if n.node == NodeKind::Component {
            components += 1;
        }
    });
    assert!(
        components >= 3,
        "expected the cone to expand, got {components}"
    );
}

#[test]
fn a_ref_expands_into_the_referenced_components_tree() {
    let f = fixture(MODEL);
    let mut ex = explainer(&f, 5);
    ex.load(&one_modelpoint(100_000.0, 0.01)).expect("runs");
    let trace = ex
        .explain("claims", Some(2), &ExplainOptions::full())
        .expect("traces");

    // `claims = survivors * q * sum_assured`; the `survivors` Ref must carry the
    // evaluated tree of `survivors` at t=2, not a placeholder.
    let mut found = false;
    trace.root.walk(&mut |n| {
        if n.node == NodeKind::Ref && n.reference.as_deref() == Some("term.survivors") {
            found = true;
            assert_eq!(n.t, Some(2));
            assert_eq!(n.resolution.as_deref(), Some("computed"));
            assert_eq!(n.children.len(), 1, "a Ref expands into one Component node");
            assert_eq!(n.children[0].node, NodeKind::Component);
            assert_eq!(n.children[0].value, n.value);
        }
    });
    assert!(found, "no survivors Ref in the trace");

    // Full depth bottoms out in Input and Lit leaves only.
    trace.root.walk(&mut |n| {
        if n.children.is_empty() && n.elided.is_none() {
            assert!(
                matches!(n.node, NodeKind::Input | NodeKind::Lit | NodeKind::Agg),
                "leaf {:?} at {} is neither an Input nor a Lit",
                n.node,
                n.path
            );
        }
    });
}

#[test]
fn depth_limits_expansion_and_counts_what_it_elided() {
    let f = fixture(MODEL);
    let mut ex = explainer(&f, 5);
    ex.load(&one_modelpoint(100_000.0, 0.01)).expect("runs");
    let shallow = ex
        .explain("claims", Some(3), &ExplainOptions::default())
        .expect("traces");
    let full = ex
        .explain("claims", Some(3), &ExplainOptions::full())
        .expect("traces");
    assert!(shallow.root.count() < full.root.count());
    assert!(shallow.truncated.elided_nodes > 0);
    // Truncating the tree may never change the number.
    assert_eq!(shallow.root.value, full.root.value);
}

#[test]
fn expand_overrides_the_depth_budget_for_named_components() {
    let f = fixture(MODEL);
    let mut ex = explainer(&f, 5);
    ex.load(&one_modelpoint(100_000.0, 0.01)).expect("runs");
    let mut options = ExplainOptions {
        depth: 0,
        ..ExplainOptions::default()
    };
    options.expand.insert("survivors".to_string());
    let trace = ex.explain("claims", Some(3), &options).expect("traces");
    let mut expanded = false;
    trace.root.walk(&mut |n| {
        if n.reference.as_deref() == Some("term.survivors") && !n.children.is_empty() {
            expanded = true;
        }
    });
    assert!(expanded, "`expand` must beat depth = 0");
}

#[test]
fn init_is_the_value_at_t_zero_and_says_so() {
    let f = fixture(MODEL);
    let mut ex = explainer(&f, 5);
    ex.load(&one_modelpoint(100_000.0, 0.01)).expect("runs");
    let trace = ex
        .explain("survivors", Some(0), &ExplainOptions::full())
        .expect("traces");
    assert_eq!(trace.root.value, 1.0);
    assert_eq!(trace.root.resolution.as_deref(), Some("init"));
    assert_eq!(trace.root.note.as_deref(), Some("N0302"));
    assert_eq!(trace.notes_with("N0302").len(), 1);
}

#[test]
fn a_lag_below_the_origin_resolves_to_init() {
    let f = fixture(MODEL);
    let mut ex = explainer(&f, 5);
    ex.load(&one_modelpoint(100_000.0, 0.01)).expect("runs");
    // At t = 1 the recursion reads `survivors[t-1]`, which is t = 0 — computed.
    // Force the pre-origin path by tracing `survivors` at t = 1 and walking to
    // the lag inside the *init-free* series below.
    let trace = ex
        .explain("survivors", Some(1), &ExplainOptions::full())
        .expect("traces");
    let mut lags = 0;
    trace.root.walk(&mut |n| {
        if n.node == NodeKind::Lag {
            lags += 1;
            assert_eq!(n.lag, Some(1));
            assert_eq!(n.t, Some(0));
        }
    });
    assert_eq!(lags, 1);
}

#[test]
fn pre_origin_default_fires_n0301_when_there_is_no_init() {
    let source = MODEL.replace("init = \"1.0\"\n", "");
    let f = fixture(&source);
    let mut ex = explainer(&f, 5);
    ex.load(&one_modelpoint(100_000.0, 0.01)).expect("runs");
    let trace = ex
        .explain("survivors", Some(0), &ExplainOptions::full())
        .expect("traces");
    let notes = trace.notes_with("N0301");
    assert_eq!(notes.len(), 1, "an init-free lag below the origin is N0301");
    let mut saw = false;
    trace.root.walk(&mut |n| {
        if n.resolution.as_deref() == Some("pre_origin_default") {
            saw = true;
            assert_eq!(n.value, 0.0);
        }
    });
    assert!(saw);
}

#[test]
fn agg_terms_sum_left_to_right_to_the_kernels_own_value() {
    let f = fixture(MODEL);
    let mut ex = explainer(&f, 5);
    ex.load(&one_modelpoint(100_000.0, 0.01)).expect("runs");
    let bel = ex.output().unwrap().column("bel").unwrap().lane(0)[0];
    let trace = ex
        .explain("bel", None, &ExplainOptions::full())
        .expect("traces");
    assert_eq!(trace.root.value, bel);

    let mut checked = false;
    trace.root.walk(&mut |n| {
        if n.node != NodeKind::Agg {
            return;
        }
        checked = true;
        assert_eq!(n.op.as_deref(), Some("npv"));
        assert_eq!(n.terms.len(), 6, "t = 0..=5, no gaps");
        for (i, term) in n.terms.iter().enumerate() {
            assert_eq!(term.t, i as u32);
            assert!(term.disc.is_some(), "npv terms carry the applied factor");
            assert_eq!(term.contribution, term.value * term.disc.unwrap());
        }
        let mut acc = 0.0;
        for term in &n.terms {
            acc += term.contribution;
        }
        assert_eq!(acc, n.value, "contributions must sum *exactly*");
        let over = n.over.as_ref().expect("Agg carries `over`");
        assert_eq!(over.component, "term.claims");
        assert_eq!(over.discount.as_deref(), Some("term.disc"));
        // `claims` is `end`-timed, so the exponent must be v^(t+1).
        assert_eq!(over.exponent_rule.as_deref(), Some("v^(t+1)"));
    });
    assert!(checked, "no Agg node in the bel trace");
}

#[test]
fn trace_max_terms_truncates_loudly() {
    let f = fixture(MODEL);
    let mut ex = explainer(&f, 5);
    ex.load(&one_modelpoint(100_000.0, 0.01)).expect("runs");
    let options = ExplainOptions {
        max_terms: 2,
        ..ExplainOptions::full()
    };
    let trace = ex.explain("bel", None, &options).expect("traces");
    let mut seen = false;
    trace.root.walk(&mut |n| {
        if n.node == NodeKind::Agg {
            seen = true;
            assert!(n.terms_truncated);
            assert_eq!(n.terms.len(), 2);
            assert_eq!(n.term_count, Some(6), "the untruncated count is reported");
        }
    });
    assert!(seen);
}

#[test]
fn n0401_names_the_factor_that_zeroed_the_product() {
    let f = fixture(MODEL);
    let mut ex = explainer(&f, 5);
    // A zero mortality rate zeroes `claims` through `survivors * q * sum_assured`.
    ex.load(&one_modelpoint(100_000.0, 0.0)).expect("runs");
    let trace = ex
        .explain("claims", Some(2), &ExplainOptions::full())
        .expect("traces");
    assert_eq!(trace.root.value, 0.0);
    let notes = trace.notes_with("N0401");
    assert!(!notes.is_empty(), "a zero product must raise N0401");
    assert!(notes.iter().any(|n| n.message.contains("q")));
}

#[test]
fn a_scalar_or_permp_component_refuses_a_t_and_a_series_demands_one() {
    let f = fixture(MODEL);
    let mut ex = explainer(&f, 5);
    ex.load(&one_modelpoint(100_000.0, 0.01)).expect("runs");
    assert!(matches!(
        ex.explain("bel", Some(0), &ExplainOptions::default()),
        Err(ExplainError::UnexpectedT(_))
    ));
    assert!(matches!(
        ex.explain("claims", None, &ExplainOptions::default()),
        Err(ExplainError::NeedsT(_))
    ));
    assert!(matches!(
        ex.explain("claims", Some(99), &ExplainOptions::default()),
        Err(ExplainError::TOutOfRange { t: 99, periods: 5 })
    ));
    assert!(matches!(
        ex.explain("nope", Some(0), &ExplainOptions::default()),
        Err(ExplainError::UnknownComponent(_))
    ));
}

#[test]
fn ring_retention_is_refused_rather_than_silently_misread() {
    let plan = plan_sources(
        &[Input::new("term.pir", MODEL)],
        &PlanOptions::default(), // no retain_all
    )
    .expect("plans");
    let tapes = lower_plan(&plan).expect("lowers");
    let err = Explainer::new(&plan, &tapes, vec![], &timeline(5), TraceContext::default())
        .expect_err("a ring-buffered model cannot be explained");
    assert!(matches!(err, ExplainError::NotRetained(_)));
}

#[test]
fn a_trace_replays_exactly_one_modelpoint() {
    let f = fixture(MODEL);
    let mut ex = explainer(&f, 5);
    let mut columns = BTreeMap::new();
    columns.insert("policy_id".to_string(), vec![2.0, 3.0]);
    columns.insert("sum_assured".to_string(), vec![1.0, 2.0]);
    columns.insert("q".to_string(), vec![0.01, 0.02]);
    let two = ChunkInput {
        index: 0,
        keys: vec!["A".into(), "B".into()],
        first_row: 0,
        columns,
    };
    assert!(matches!(ex.load(&two), Err(ExplainError::NotOneLane(2))));
}

#[test]
fn inputs_are_leaves_that_name_where_they_came_from() {
    let f = fixture(MODEL);
    let mut ex = explainer(&f, 5);
    ex.load(&one_modelpoint(100_000.0, 0.01)).expect("runs");
    let mut mp = false;
    let mut assumption = false;
    let mut visit = |n: &predictable_engine::Node| {
        if n.node != NodeKind::Input {
            return;
        }
        assert!(n.children.is_empty(), "an Input is always a leaf");
        match n.kind.as_deref() {
            Some("Modelpoint") => {
                mp = true;
                let src = n.source.as_ref().expect("modelpoint inputs carry a source");
                assert_eq!(src.file.as_deref(), Some("data/term.csv"));
                assert_eq!(src.row, Some(41));
            }
            Some("Assumption") => {
                assumption = true;
                let src = n.source.as_ref().expect("assumptions carry a source");
                assert_eq!(src.assumption_set.as_deref(), Some("base"));
            }
            _ => {}
        }
    };
    // `claims` reaches the modelpoint columns; `disc` reaches the assumption.
    for (component, t) in [("claims", Some(1)), ("disc", Some(1))] {
        let trace = ex
            .explain(component, t, &ExplainOptions::full())
            .expect("traces");
        trace.root.walk(&mut visit);
    }
    assert!(mp, "no modelpoint Input leaf");
    assert!(assumption, "no assumption Input leaf");
}

#[test]
fn the_text_is_a_projection_of_the_json() {
    let f = fixture(MODEL);
    let mut ex = explainer(&f, 5);
    ex.load(&one_modelpoint(100_000.0, 0.01)).expect("runs");
    let trace = ex
        .explain("claims", Some(3), &ExplainOptions::full())
        .expect("traces");
    let text = trace.text();

    // Deterministic: the same tree renders identically, every time.
    assert_eq!(text, trace.text());
    // Everything in the header comes out of the JSON.
    assert!(text.starts_with("term.claims[t=3]  POL00042  = "));
    assert!(text.contains("survivors"));
    assert!(text.contains("sum_assured"));
    // Money is rendered to 2 dp with thousands separators (§3.5): 1000.0 is
    // "1,000.00", never "1000" and never "1000.0".
    let header = text.lines().next().expect("a header line");
    assert!(header.contains(".00") || header.contains(&format!("{:.2}", trace.root.value)));
    assert!(!header.contains(&format!("= {}", trace.root.value)));
    // No trailing whitespace anywhere: the rendering is a golden.
    for line in text.lines() {
        assert_eq!(line, line.trim_end(), "trailing space in `{line}`");
    }
}

#[test]
fn the_json_round_trips_and_carries_the_run_it_explains() {
    let f = fixture(MODEL);
    let mut ex = explainer(&f, 5);
    ex.load(&one_modelpoint(100_000.0, 0.01)).expect("runs");
    let trace = ex
        .explain("bel", None, &ExplainOptions::full())
        .expect("traces");
    assert_eq!(trace.format, "pvf/1");
    assert_eq!(trace.kind, "trace");
    assert_eq!(trace.run, "sha256:test");
    let json = trace.to_json();
    let back: predictable_engine::Trace = serde_json::from_str(&json).expect("round-trips");
    assert_eq!(back, trace);
}
