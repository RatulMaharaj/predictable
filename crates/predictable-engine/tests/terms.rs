//! Sequential reductions (`01-ir.md` §9.2) and the per-`t` term retention that
//! `explain()` replays over (`01-ir.md` §11.2, decision Q11).

mod common;

use std::collections::BTreeMap;

use common::{chunk, model, Model};
use predictable_engine::{ChunkInput, Engine, RunConfig};

const NPV: &str = r#"
format = "pir/1"
module = "t"

[timeline]
basis = "annual"
periods = 3
origin = "policy"
valuation_date = 2026-06-30

[[modelpoint_field]]
name = "policy_number"
dtype = "str"
required = true
key = true

[[modelpoint_field]]
name = "premium"
dtype = "f64"
unit = "money"
required = true

[[component]]
name = "disc"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "factor"
timing = "start"
init = "1.0"
expr = "disc[t-1] * 0.96"

[[component]]
name = "premium_income"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "TIMING"
expr = "premium * 1.0"

[[component]]
name = "pv_premiums"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "npv(premium_income, disc)"
"#;

/// Run one chunk and keep the engine and its arena alive, because a trace is a
/// *replay* over the buffers the run left behind.
struct Ran<'a> {
    engine: Engine<'a>,
    bufs: predictable_engine::ChunkBuffers,
    out: predictable_engine::ChunkOutput,
}

fn run_keeping_buffers<'a>(m: &'a Model, input: &ChunkInput) -> Ran<'a> {
    let mut engine =
        Engine::new(&m.plan, &m.tapes, vec![], &m.timeline, RunConfig::default()).expect("engine");
    let mut bufs = engine.buffers();
    engine
        .prepare(&BTreeMap::new(), &mut bufs)
        .expect("prepare");
    let out = engine.run_chunk(&mut bufs, input).expect("run");
    Ran { engine, bufs, out }
}

#[test]
fn npv_terms_show_the_discount_exponent() {
    let m = model(&NPV.replace("TIMING", "start"));
    let ran = run_keeping_buffers(&m, &chunk(0, &[("premium", vec![500.0])]));
    let trace = ran
        .engine
        .agg_trace_default(&ran.bufs, "pv_premiums", 0)
        .expect("trace");

    assert_eq!(trace.node, "Agg");
    assert_eq!(trace.op, "npv");
    assert_eq!(trace.reference, "t.premium_income");
    assert_eq!(trace.term_count, 4);
    assert_eq!(trace.terms.len(), 4);
    assert!(!trace.terms_truncated);

    // `timing = "start"` ⇒ exponent `t` ⇒ the cumulative factor as it stands.
    for (i, term) in trace.terms.iter().enumerate() {
        assert_eq!(term.t, i as u32);
        assert_eq!(term.value, 500.0);
        assert_eq!(term.disc, Some(0.96f64.powi(i as i32)));
        assert_eq!(term.contribution, term.value * term.disc.unwrap());
    }

    // Q11's hard requirement: the contributions re-add to the value exactly,
    // and the value is the one the kernel emitted.
    assert!(trace.sums_exactly());
    assert_eq!(
        trace.value,
        ran.out.column("pv_premiums").unwrap().lane(0)[0]
    );
}

/// The exponent is the bug that hides: `end` timing must discount a period
/// further than `start`, and the trace must show *which*.
#[test]
fn timing_changes_the_recorded_factor() {
    let start = model(&NPV.replace("TIMING", "start"));
    let end = model(&NPV.replace("TIMING", "end"));
    let input = chunk(0, &[("premium", vec![500.0])]);

    let a = run_keeping_buffers(&start, &input);
    let b = run_keeping_buffers(&end, &input);
    let ta = a
        .engine
        .agg_trace_default(&a.bufs, "pv_premiums", 0)
        .unwrap();
    let tb = b
        .engine
        .agg_trace_default(&b.bufs, "pv_premiums", 0)
        .unwrap();

    assert_eq!(format!("{:?}", ta.timing_used), "Some(Start)");
    assert_eq!(format!("{:?}", tb.timing_used), "Some(End)");
    // v^(t+1) = v^t · v, taken from the curve's own one-step factor. The last
    // period has no next point on the curve, so it keeps its own factor —
    // documented behaviour, and visible in the trace rather than implied.
    for (x, y) in ta.terms.iter().zip(tb.terms.iter()).take(3) {
        let ratio = y.disc.unwrap() / x.disc.unwrap();
        assert!((ratio - 0.96).abs() < 1e-12, "t = {}: {ratio}", x.t);
    }
    assert_eq!(ta.terms[3].disc, tb.terms[3].disc);
    assert!(tb.value < ta.value);
}

const SUMS: &str = r#"
format = "pir/1"
module = "s"

[timeline]
basis = "annual"
periods = 5
origin = "policy"
valuation_date = 2026-06-30

[[modelpoint_field]]
name = "policy_number"
dtype = "str"
required = true
key = true

[[modelpoint_field]]
name = "big"
dtype = "f64"
unit = "money"
required = true

[[modelpoint_field]]
name = "small"
dtype = "f64"
unit = "money"
required = true

[[component]]
name = "flow"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "if t < 1 then big else small"

[[component]]
name = "in_window"
kind = "Derived"
dtype = "bool"
shape = "Series"
timing = "point"
expr = "t < 3"

[[component]]
name = "total"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum(flow)"

[[component]]
name = "windowed"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum(flow, in_window)"

[[component]]
name = "compensated"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum_kahan(flow)"

[[component]]
name = "biggest"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "max_over(flow)"

[[component]]
name = "run_length"
kind = "Output"
dtype = "f64"
shape = "PerMP"
expr = "count_while(in_window)"
"#;

/// Adversarial magnitudes: `1e16` alternating with `1.0` is exactly where any
/// reassociation — pairwise, SIMD, or a "harmless" reordering — shows up.
#[test]
fn reductions_are_strictly_sequential_in_t() {
    let m = model(SUMS);
    let input = chunk(0, &[("big", vec![1e16]), ("small", vec![1.0])]);
    let ran = run_keeping_buffers(&m, &input);

    // One value far larger than the rest: added first, every later `1.0`
    // vanishes below the ulp — added in pairs first, they survive.
    let flow: Vec<f64> = (0..=5).map(|t| if t < 1 { 1e16 } else { 1.0 }).collect();

    // The reference: left to right, no compensation.
    let mut sequential = 0.0;
    for x in &flow {
        sequential += x;
    }
    // What a pairwise/tree sum would give — deliberately different.
    let pairwise = ((flow[0] + flow[1]) + (flow[2] + flow[3])) + (flow[4] + flow[5]);
    assert_ne!(sequential, pairwise, "the test data must be adversarial");

    assert_eq!(ran.out.column("total").unwrap().lane(0)[0], sequential);

    // `sum_kahan` is the opt-in compensated variant: the model has to ask for
    // it, and then it recovers the lost bits — still walking `t` in order.
    let compensated = ran.out.column("compensated").unwrap().lane(0)[0];
    let (mut acc, mut c) = (0.0f64, 0.0f64);
    for x in &flow {
        let sum = acc + x;
        c += if acc.abs() >= x.abs() {
            (acc - sum) + x
        } else {
            (x - sum) + acc
        };
        acc = sum;
    }
    assert_eq!(compensated, acc + c);
    assert_ne!(compensated, sequential);

    // And the recorder reproduces the kernel's own value, bit for bit.
    let trace = ran.engine.agg_trace_default(&ran.bufs, "total", 0).unwrap();
    assert_eq!(trace.value, sequential);
    assert!(trace.sums_exactly());
    assert_eq!(
        trace.terms.iter().map(|t| t.value).collect::<Vec<_>>(),
        flow
    );
}

#[test]
fn predicated_terms_carry_their_inclusion() {
    let m = model(SUMS);
    let ran = run_keeping_buffers(&m, &chunk(0, &[("big", vec![10.0]), ("small", vec![1.0])]));
    let trace = ran
        .engine
        .agg_trace_default(&ran.bufs, "windowed", 0)
        .unwrap();

    assert_eq!(trace.op, "sum");
    assert_eq!(trace.term_count, 6);
    let included: Vec<bool> = trace.terms.iter().map(|t| t.included).collect();
    assert_eq!(included, [true, true, true, false, false, false]);
    for term in &trace.terms {
        assert_eq!(
            term.contribution,
            if term.included { term.value } else { 0.0 }
        );
    }
    assert!(trace.sums_exactly());
    assert_eq!(trace.value, ran.out.column("windowed").unwrap().lane(0)[0]);
}

#[test]
fn count_while_marks_where_it_stopped() {
    let m = model(SUMS);
    let ran = run_keeping_buffers(&m, &chunk(0, &[("big", vec![10.0]), ("small", vec![1.0])]));
    let trace = ran
        .engine
        .agg_trace_default(&ran.bufs, "run_length", 0)
        .unwrap();

    assert_eq!(trace.op, "count_while");
    assert_eq!(trace.value, 3.0);
    // It stops at the first false rather than counting every true period.
    assert_eq!(trace.terms.len(), 4);
    assert!(trace.terms[3].stopped_here);
    assert!(!trace.terms[3].included);
    assert!(trace.sums_exactly());
}

#[test]
fn selecting_aggregates_mark_the_chosen_period() {
    let m = model(SUMS);
    let ran = run_keeping_buffers(&m, &chunk(0, &[("big", vec![10.0]), ("small", vec![1.0])]));
    let trace = ran
        .engine
        .agg_trace_default(&ran.bufs, "biggest", 0)
        .unwrap();
    assert_eq!(trace.op, "max_over");
    assert_eq!(trace.value, 10.0);
    let chosen: Vec<u32> = trace
        .terms
        .iter()
        .filter(|t| t.contribution != 0.0)
        .map(|t| t.t)
        .collect();
    assert_eq!(chosen, vec![0]);
    assert!(trace.sums_exactly());
}

/// `--trace-max-terms`: truncation keeps the ends and says so, never silently.
#[test]
fn truncation_is_loud() {
    let m = model(SUMS);
    let ran = run_keeping_buffers(&m, &chunk(0, &[("big", vec![10.0]), ("small", vec![1.0])]));
    let full = ran.engine.agg_trace_default(&ran.bufs, "total", 0).unwrap();
    let cut = ran.engine.agg_trace(&ran.bufs, "total", 0, 4).unwrap();

    assert_eq!(full.terms.len(), 6);
    assert!(!full.terms_truncated);
    assert!(cut.terms_truncated);
    assert_eq!(cut.term_count, 6, "the untruncated count is still reported");
    assert_eq!(cut.terms.len(), 4);
    assert_eq!(
        cut.terms.iter().map(|t| t.t).collect::<Vec<_>>(),
        vec![0, 1, 4, 5]
    );
    assert_eq!(cut.value, full.value);
}

/// The trace is the JSON of `01-ir.md` §11.2, so `explain()` (T23) can nest it
/// as an `Agg` node verbatim.
#[test]
fn a_trace_serialises_to_the_spec_shape() {
    let m = model(&NPV.replace("TIMING", "start"));
    let ran = run_keeping_buffers(&m, &chunk(0, &[("premium", vec![500.0])]));
    let trace = ran
        .engine
        .agg_trace_default(&ran.bufs, "pv_premiums", 0)
        .unwrap();
    let json: serde_json::Value = serde_json::to_value(&trace).unwrap();

    assert_eq!(json["node"], "Agg");
    assert_eq!(json["op"], "npv");
    assert_eq!(json["ref"], "t.premium_income");
    assert_eq!(json["timing_used"], "start");
    assert_eq!(json["term_count"], 4);
    let first = &json["terms"][0];
    assert_eq!(first["t"], 0);
    assert_eq!(first["value"], 500.0);
    assert_eq!(first["disc"], 1.0);
    assert_eq!(first["contribution"], 500.0);
    // Unpredicated terms do not carry noise fields.
    assert!(first.get("included").is_none());
    assert!(first.get("stopped_here").is_none());
    assert!(json.get("terms_truncated").is_none());
}

/// An unknown or non-aggregate component is `None`, never a fabricated trace.
#[test]
fn only_aggregates_have_terms() {
    let m = model(&NPV.replace("TIMING", "start"));
    let ran = run_keeping_buffers(&m, &chunk(0, &[("premium", vec![500.0])]));
    assert!(ran.engine.agg_trace_default(&ran.bufs, "nope", 0).is_none());
    assert!(ran
        .engine
        .agg_trace_default(&ran.bufs, "premium_income", 0)
        .is_none());
}

/// The recorder is replay-only: it reads the arena, and reading it twice gives
/// the same answer as reading it once (nothing is consumed or mutated).
#[test]
fn tracing_does_not_disturb_the_run() {
    let m = model(&NPV.replace("TIMING", "end"));
    let ran = run_keeping_buffers(&m, &chunk(0, &[("premium", vec![500.0, 700.0])]));
    for lane in 0..2 {
        let a = ran
            .engine
            .agg_trace_default(&ran.bufs, "pv_premiums", lane)
            .unwrap();
        let b = ran
            .engine
            .agg_trace_default(&ran.bufs, "pv_premiums", lane)
            .unwrap();
        assert_eq!(a, b);
        assert_eq!(
            a.value,
            ran.out.column("pv_premiums").unwrap().lane(lane)[0]
        );
    }
}
