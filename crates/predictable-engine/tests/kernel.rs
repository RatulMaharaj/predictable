//! Behavioural tests for the kernel: values, layout, arena reuse, chunk
//! invariance, and the trap path end to end.

mod common;

use std::collections::BTreeMap;

use common::{chunk, model, run, run_simple};
use predictable_engine::{ChunkInput, Engine, EngineError, Place, RunConfig, TrapKind, TrapPolicy};

const TERM: &str = r#"
format = "pir/1"
module = "term"

[timeline]
basis = "annual"
periods = 5
origin = "policy"
valuation_date = 2026-06-30

[[assumption]]
name = "valuation_rate"
dtype = "f64"
unit = "rate(annual)"
shape = "Scalar"

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

[[modelpoint_field]]
name = "q"
dtype = "f64"
unit = "prob"
required = true

[[component]]
name = "disc"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "factor"
timing = "start"
init = "1.0"
expr = "disc[t-1] * v_from_i(valuation_rate)"
doc = "Depends only on the timeline and a scalar, so the planner hoists it."

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
timing = "start"
expr = "survivors * q * sum_assured"

[[component]]
name = "pv_claims"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "npv(claims, disc)"
"#;

fn term_chunk(index: u32, n: usize) -> ChunkInput {
    let sums: Vec<f64> = (0..n).map(|i| 100_000.0 + 1000.0 * i as f64).collect();
    let qs: Vec<f64> = (0..n).map(|i| 0.01 + 0.001 * i as f64).collect();
    chunk(index, &[("sum_assured", sums), ("q", qs)])
}

fn assumptions() -> BTreeMap<String, f64> {
    BTreeMap::from([("valuation_rate".to_string(), 0.04)])
}

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

#[test]
fn projects_a_term_model_by_hand() {
    let m = model(TERM);
    let out = run(
        &m,
        vec![],
        RunConfig::default(),
        &assumptions(),
        &[term_chunk(0, 2)],
    )
    .expect("run");
    let claims = out[0].column("term.claims").expect("claims column");

    // Lane 0: q = 0.01, SA = 100_000, survivors run off at (1 - q) per period.
    let (q, sa) = (0.01, 100_000.0);
    let mut survivors = 1.0;
    for t in 0..=5usize {
        assert_eq!(
            claims.lane(0)[t],
            survivors * q * sa,
            "claims at t = {t} on lane 0"
        );
        survivors *= 1.0 - q;
    }

    // The npv uses `claims`' timing (`start` → `v^t`) against the hoisted
    // discount series, sequentially in t.
    let v: f64 = 1.0 / 1.04;
    let mut expected = 0.0;
    let mut survivors = 1.0;
    for t in 0..=5 {
        expected += survivors * q * sa * v.powi(t);
        survivors *= 1.0 - q;
    }
    let pv = out[0].column("term.pv_claims").expect("pv column");
    assert_eq!(pv.lane(0)[0], expected);
}

#[test]
fn lanes_are_independent() {
    let m = model(TERM);
    let out = run(
        &m,
        vec![],
        RunConfig::default(),
        &assumptions(),
        &[term_chunk(0, 8)],
    )
    .expect("run");
    let claims = out[0].column("term.claims").unwrap();
    for lane in 0..8 {
        let (q, sa) = (0.01 + 0.001 * lane as f64, 100_000.0 + 1000.0 * lane as f64);
        assert_eq!(claims.lane(lane)[0], q * sa);
    }
}

// ---------------------------------------------------------------------------
// Chunking must not be observable (§7)
// ---------------------------------------------------------------------------

#[test]
fn chunk_size_one_equals_chunk_size_1024_bit_for_bit() {
    let m = model(TERM);
    let wide = run(
        &m,
        vec![],
        RunConfig::default(),
        &assumptions(),
        &[term_chunk(0, 40)],
    )
    .expect("wide run");

    let narrow_chunks: Vec<ChunkInput> = (0..40)
        .map(|i| {
            let mut c = term_chunk(0, 40);
            c.index = i;
            c.keys = vec![c.keys[i as usize].clone()];
            c.first_row = u64::from(i);
            for v in c.columns.values_mut() {
                *v = vec![v[i as usize]];
            }
            c
        })
        .collect();
    let narrow = run(
        &m,
        vec![],
        RunConfig {
            chunk: 1,
            ..RunConfig::default()
        },
        &assumptions(),
        &narrow_chunks,
    )
    .expect("narrow run");

    let wide_claims = wide[0].column("term.claims").unwrap();
    for (lane, single) in narrow.iter().enumerate() {
        let one = single.column("term.claims").unwrap();
        assert_eq!(
            wide_claims
                .lane(lane)
                .iter()
                .map(|x| x.to_bits())
                .collect::<Vec<_>>(),
            one.lane(0).iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
            "lane {lane} differs between C = 1024 and C = 1"
        );
    }
}

#[test]
fn the_arena_is_reused_without_leaking_state() {
    let m = model(TERM);
    let mut engine =
        Engine::new(&m.plan, &m.tapes, vec![], &m.timeline, RunConfig::default()).unwrap();
    let mut bufs = engine.buffers();
    engine.prepare(&assumptions(), &mut bufs).unwrap();

    // Two chunks of different widths through the same buffers, then the second
    // chunk again through fresh buffers: the reused arena must agree.
    let a = engine.run_chunk(&mut bufs, &term_chunk(0, 7)).unwrap();
    let b = engine.run_chunk(&mut bufs, &term_chunk(1, 3)).unwrap();
    assert_eq!(a.keys.len(), 7);

    let mut fresh = engine.buffers();
    engine.prepare(&assumptions(), &mut fresh).unwrap();
    let b2 = engine.run_chunk(&mut fresh, &term_chunk(1, 3)).unwrap();
    assert_eq!(b.column("term.claims"), b2.column("term.claims"));
    assert_eq!(b.column("term.pv_claims"), b2.column("term.pv_claims"));
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

#[test]
fn retention_decides_the_footprint() {
    let m = model(TERM);
    let engine = Engine::new(&m.plan, &m.tapes, vec![], &m.timeline, RunConfig::default()).unwrap();
    let layout = engine.layout();

    let survivors = m.plan.series_named("survivors").unwrap().info.id;
    let claims = m.plan.series_named("claims").unwrap().info.id;
    let disc = m.plan.series_named("disc").unwrap().info.id;

    // `survivors` is lagged by 1 and read by nothing else, so it rings.
    match layout.place(survivors) {
        Place::Series { periods, full, .. } => {
            assert!(!full);
            assert_eq!(periods, 2, "ring of max_lag + 1, rounded to a power of two");
        }
        other => panic!("survivors is {other:?}"),
    }
    // `claims` is an output and an npv argument, so it is retained in full.
    match layout.place(claims) {
        Place::Series { periods, full, .. } => {
            assert!(full);
            assert_eq!(periods, 6);
        }
        other => panic!("claims is {other:?}"),
    }
    // `disc` touches no modelpoint value, so it left the loop entirely.
    assert!(matches!(layout.place(disc), Place::Hoisted { .. }));
    // Hoisted storage has no modelpoint axis: `T+1` per slot, not `(T+1) × C`.
    let hoisted_slots = layout.hoisted_len / 6;
    assert_eq!(layout.hoisted_len, hoisted_slots * 6);
    assert!(hoisted_slots >= 1);
    // The whole hoisted set is smaller than a single chunked series lane.
    assert!(layout.hoisted_len < layout.chunk);
}

// ---------------------------------------------------------------------------
// Traps
// ---------------------------------------------------------------------------

const DIVIDER: &str = r#"
format = "pir/1"
module = "trap"

[timeline]
basis = "annual"
periods = 2
origin = "policy"
valuation_date = 2026-06-30

[[modelpoint_field]]
name = "policy_number"
dtype = "str"
required = true
key = true

[[modelpoint_field]]
name = "num"
dtype = "f64"
unit = "money"
required = true

[[modelpoint_field]]
name = "den"
dtype = "f64"
unit = "money"
required = true

[[component]]
name = "ratio"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "factor"
timing = "start"
expr = "num / den"
"#;

fn div_chunk(dens: Vec<f64>) -> ChunkInput {
    let n = dens.len();
    chunk(0, &[("num", vec![1.0; n]), ("den", dens)])
}

#[test]
fn a_division_by_zero_aborts_with_an_e0902_envelope() {
    let m = model(DIVIDER);
    let err = run(
        &m,
        vec![],
        RunConfig::default(),
        &BTreeMap::new(),
        &[div_chunk(vec![2.0, 0.0, 4.0])],
    )
    .expect_err("must trap");

    let EngineError::Trapped(log) = err else {
        panic!("expected a trap abort");
    };
    assert_eq!(log.total(), 1);
    let report = &log.reports()[0];
    assert_eq!(report.kind, TrapKind::DivByZero);
    assert_eq!(report.component, "trap.ratio");
    assert_eq!(
        report.mp_key, "MP1",
        "the trap names the offending modelpoint"
    );
    assert_eq!(report.mp_row, 1);
    assert_eq!(report.t, 0);
    // The operands come from the scalar replay, so they are the real values.
    assert_eq!(
        report.operands,
        vec![("lhs".into(), 1.0), ("rhs".into(), 0.0)]
    );

    let envelope = report.envelope();
    assert_eq!(envelope["code"], "E0902");
    assert_eq!(envelope["kind"], "trap");
    assert_eq!(envelope["trap"], "div_by_zero");
    assert_eq!(envelope["expr_path"], "expr");
    assert_eq!(envelope["operands"]["rhs"], 0.0);
    assert_eq!(
        envelope["doc_url"],
        "https://predictable.dev/diagnostics/E0902"
    );
}

#[test]
fn continue_on_trap_drops_the_modelpoint_and_keeps_the_rest() {
    let m = model(DIVIDER);
    let out = run(
        &m,
        vec![],
        RunConfig {
            on_trap: TrapPolicy::Continue,
            ..RunConfig::default()
        },
        &BTreeMap::new(),
        &[div_chunk(vec![2.0, 0.0, 4.0])],
    )
    .expect("continue does not abort");

    let out = &out[0];
    // No null rows (`01-ir.md` §2.11): the modelpoint contributes nothing.
    assert_eq!(out.keys, vec!["MP0".to_string(), "MP2".to_string()]);
    assert_eq!(out.dropped, vec!["MP1".to_string()]);
    let ratio = out.column("trap.ratio").unwrap();
    assert_eq!(ratio.lanes(), 2);
    assert_eq!(ratio.lane(0)[0], 0.5);
    assert_eq!(ratio.lane(1)[0], 0.25);
    assert_eq!(out.traps.modelpoints(), 1);
}

#[test]
fn max_errors_caps_reports_but_never_counts() {
    let m = model(DIVIDER);
    let out = run(
        &m,
        vec![],
        RunConfig {
            on_trap: TrapPolicy::Continue,
            max_errors: 2,
            ..RunConfig::default()
        },
        &BTreeMap::new(),
        &[div_chunk(vec![0.0, 0.0, 0.0, 0.0, 1.0])],
    )
    .expect("continue");

    let traps = &out[0].traps;
    assert_eq!(traps.total(), 4, "counts stay exact");
    assert_eq!(traps.reports().len(), 2, "retention is capped");
    assert!(traps.truncated());
    // Reports are the *first* offenders in lane order, so the set is stable.
    assert_eq!(traps.reports()[0].mp_key, "MP0");
    assert_eq!(traps.reports()[1].mp_key, "MP1");
}

#[test]
fn a_trap_in_an_untaken_if_arm_is_suppressed() {
    // `01-ir.md` §2.6: both arms are evaluated, so the division happens on every
    // lane — the mask is what stops it being reported.
    let src = DIVIDER.replace(
        r#"expr = "num / den""#,
        r#"expr = "if den == 0.0 then 0.0 else num / den""#,
    );
    let m = model(&src);
    let out = run_simple(&m, &[div_chunk(vec![2.0, 0.0, 4.0])]);
    let ratio = out[0].column("trap.ratio").unwrap();
    assert_eq!(ratio.lane(0)[0], 0.5);
    assert_eq!(ratio.lane(1)[0], 0.0, "the guarded lane took the safe arm");
    assert!(out[0].traps.is_empty());
    assert!(out[0].dropped.is_empty());
}

#[test]
fn a_missing_modelpoint_column_is_an_error_before_any_arithmetic() {
    let m = model(DIVIDER);
    let err = run(
        &m,
        vec![],
        RunConfig::default(),
        &BTreeMap::new(),
        &[chunk(0, &[("num", vec![1.0])])],
    )
    .expect_err("den is missing");
    assert!(matches!(err, EngineError::MissingColumn { .. }), "{err}");
}

#[test]
fn a_missing_assumption_is_an_error_before_any_arithmetic() {
    let m = model(TERM);
    let err = run(
        &m,
        vec![],
        RunConfig::default(),
        &BTreeMap::new(),
        &[term_chunk(0, 1)],
    )
    .expect_err("valuation_rate is missing");
    assert!(
        matches!(err, EngineError::MissingAssumption { .. }),
        "{err}"
    );
}

#[test]
fn a_chunk_wider_than_the_arena_is_refused() {
    let m = model(DIVIDER);
    let err = run(
        &m,
        vec![],
        RunConfig {
            chunk: 2,
            ..RunConfig::default()
        },
        &BTreeMap::new(),
        &[div_chunk(vec![1.0, 2.0, 3.0])],
    )
    .expect_err("three lanes into a two-lane arena");
    assert!(matches!(err, EngineError::ChunkOverflow { .. }), "{err}");
}
