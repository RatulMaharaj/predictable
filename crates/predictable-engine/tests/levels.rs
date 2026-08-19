//! Substage levelling: the stage-2 → `init` back-channel (`03-engine.md` §5.3,
//! `01-ir.md` §8.2), and the `W0110` lint above three levels.

mod common;

use common::{chunk, model, run_simple};
use predictable_engine::{Engine, Levels, RunConfig};
use predictable_plan::Retention;

/// The reference shape of §5.3: `reserve` is seeded prospectively from `bel`,
/// an `npv` over a series computed in the same projection.
const BACK_CHANNEL: &str = r#"
format = "pir/1"
module = "ref"

[timeline]
basis = "annual"
periods = 4
origin = "policy"
valuation_date = 2026-06-30

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
expr = "disc[t-1] * 0.95"

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
name = "bel"
kind = "Derived"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "npv(claims, disc)"

[[component]]
name = "reserve"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
init = "bel"
expr = "reserve[t-1] * 1.05 - claims"

[[component]]
name = "pv_reserve"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum(reserve)"
"#;

/// The scalar reference: what the model means, written out by hand.
fn expected(sum_assured: f64, q: f64, periods: usize) -> (Vec<f64>, f64, Vec<f64>, f64) {
    let mut survivors = vec![1.0];
    for t in 1..=periods {
        survivors.push(survivors[t - 1] * (1.0 - q));
    }
    let claims: Vec<f64> = survivors.iter().map(|s| s * q * sum_assured).collect();
    let mut disc = vec![1.0];
    for t in 1..=periods {
        disc.push(disc[t - 1] * 0.95);
    }
    // `claims` is `timing = "start"`, so the exponent is `t` and the factor is
    // the cumulative curve as it stands.
    let mut bel = 0.0;
    for t in 0..=periods {
        bel += claims[t] * disc[t];
    }
    let mut reserve = vec![bel];
    for t in 1..=periods {
        reserve.push(reserve[t - 1] * 1.05 - claims[t]);
    }
    let mut pv = 0.0;
    for r in &reserve {
        pv += r;
    }
    (claims, bel, reserve, pv)
}

#[test]
fn back_channel_needs_exactly_two_levels() {
    let m = model(BACK_CHANNEL);
    let levels = Levels::analyze(&m.plan, &m.tapes);
    assert_eq!(levels.count(), 2, "{}", levels.text(&m.plan));
    assert!(!levels.is_flat());

    let name = |s: &predictable_plan::SlotId| m.plan.info(*s).qualified_id();
    let level0: Vec<String> = levels.stage1(0).iter().map(name).collect();
    let level1: Vec<String> = levels.stage1(1).iter().map(name).collect();
    assert!(level0.contains(&"ref.survivors".to_string()));
    assert!(level0.contains(&"ref.claims".to_string()));
    assert_eq!(level1, vec!["ref.reserve".to_string()]);

    // `bel` is reducible from level-0 series, so it runs after level 0's loop;
    // `pv_reserve` reduces a level-1 series and must wait for level 1.
    let s2_0: Vec<String> = levels.stage2(0).iter().map(name).collect();
    let s2_1: Vec<String> = levels.stage2(1).iter().map(name).collect();
    assert_eq!(s2_0, vec!["ref.bel".to_string()]);
    assert_eq!(s2_1, vec!["ref.pv_reserve".to_string()]);

    // The back-channel itself is reported, not inferred by the caller.
    let reserve = m.plan.series_named("reserve").unwrap().info.id;
    let bel = m.plan.permp_named("bel").unwrap().info.id;
    assert_eq!(
        levels
            .back_channel()
            .get(&reserve)
            .map(|s| s.iter().copied().collect::<Vec<_>>()),
        Some(vec![bel])
    );
}

#[test]
fn a_model_without_a_back_channel_is_flat() {
    let m = model(&BACK_CHANNEL.replace("init = \"bel\"", "init = \"0.0\""));
    let levels = Levels::analyze(&m.plan, &m.tapes);
    assert!(levels.is_flat());
    assert_eq!(levels.count(), 1);
    assert!(levels.back_channel().is_empty());
    assert!(levels.cross_level_reads().is_empty());
}

#[test]
fn levelled_run_matches_the_scalar_reference() {
    let m = model(BACK_CHANNEL);
    let out = run_simple(
        &m,
        &[chunk(
            0,
            &[
                ("sum_assured", vec![100_000.0, 250_000.0]),
                ("q", vec![0.01, 0.02]),
            ],
        )],
    );
    let out = &out[0];
    for (lane, (sa, q)) in [(100_000.0, 0.01), (250_000.0, 0.02)].iter().enumerate() {
        let (claims, bel, reserve, pv) = expected(*sa, *q, 4);
        assert_eq!(out.column("claims").unwrap().lane(lane), claims.as_slice());
        assert_eq!(
            out.column("reserve").unwrap().lane(lane),
            reserve.as_slice()
        );
        assert_eq!(out.column("pv_reserve").unwrap().lane(lane), [pv]);
        // `reserve[0]` *is* the seed, i.e. the stage-2 value flowed backwards.
        assert_eq!(out.column("reserve").unwrap().lane(lane)[0], bel);
    }
}

#[test]
fn levelling_does_not_change_chunk_invariance() {
    let m = model(BACK_CHANNEL);
    let sa = vec![100_000.0, 250_000.0, 50_000.0];
    let q = vec![0.01, 0.02, 0.005];
    let wide = run_simple(
        &m,
        &[chunk(0, &[("sum_assured", sa.clone()), ("q", q.clone())])],
    );
    let narrow: Vec<_> = (0..3)
        .map(|i| {
            run_simple(
                &m,
                &[chunk(
                    i as u32,
                    &[("sum_assured", vec![sa[i]]), ("q", vec![q[i]])],
                )],
            )
        })
        .collect();
    for (i, one) in narrow.iter().enumerate() {
        for col in ["claims", "reserve", "pv_reserve"] {
            assert_eq!(
                wide[0].column(col).unwrap().lane(i),
                one[0].column(col).unwrap().lane(0),
                "{col} lane {i}"
            );
        }
    }
}

/// A ring-retained series read from a higher level would have wrapped away by
/// the time that level runs, so the engine widens it to `Full` (§4.3, §5.3).
#[test]
fn cross_level_reads_are_retained_in_full() {
    let m = model(BACK_CHANNEL);
    let levels = Levels::analyze(&m.plan, &m.tapes);
    let claims = m.plan.series_named("claims").unwrap();
    // `claims` is an output, so the planner already retains it in full; the
    // interesting slot is `survivors`, which is not read across levels.
    assert!(claims.retention.is_full());
    let survivors = m.plan.series_named("survivors").unwrap();
    assert!(matches!(survivors.retention, Retention::Ring { .. }));
    assert!(!levels.cross_level_reads().contains(&survivors.info.id));

    // Ring storage stays ring storage: levelling is not an excuse to retain
    // everything.
    let engine =
        Engine::new(&m.plan, &m.tapes, vec![], &m.timeline, RunConfig::default()).expect("engine");
    match engine.layout().place(survivors.info.id) {
        predictable_engine::Place::Series { full, .. } => assert!(!full),
        p => panic!("unexpected place {p:?}"),
    }
}

/// A level-0 series that only a level-1 component reads: the planner would give
/// it a ring, and the engine must widen it or the level-1 read is garbage.
#[test]
fn a_ring_read_across_levels_is_widened_and_correct() {
    let src = BACK_CHANNEL.replace(
        r#"expr = "reserve[t-1] * 1.05 - claims""#,
        r#"expr = "reserve[t-1] * 1.05 - claims + survivors[t-1] * sum_assured""#,
    );
    let m = model(&src);
    let levels = Levels::analyze(&m.plan, &m.tapes);
    let survivors = m.plan.series_named("survivors").unwrap();
    assert!(matches!(survivors.retention, Retention::Ring { .. }));
    assert!(levels.cross_level_reads().contains(&survivors.info.id));

    let engine =
        Engine::new(&m.plan, &m.tapes, vec![], &m.timeline, RunConfig::default()).expect("engine");
    match engine.layout().place(survivors.info.id) {
        predictable_engine::Place::Series { full, periods, .. } => {
            assert!(full);
            assert_eq!(periods, m.plan.periods as usize + 1);
        }
        p => panic!("unexpected place {p:?}"),
    }

    let out = run_simple(
        &m,
        &[chunk(
            0,
            &[("sum_assured", vec![100_000.0]), ("q", vec![0.01])],
        )],
    );
    let (claims, bel, _, _) = expected(100_000.0, 0.01, 4);
    let mut survivors_ref = vec![1.0];
    for t in 1..=4 {
        survivors_ref.push(survivors_ref[t - 1] * 0.99);
    }
    let mut reserve = vec![bel];
    for t in 1..=4 {
        reserve.push(reserve[t - 1] * 1.05 - claims[t] + survivors_ref[t - 1] * 100_000.0);
    }
    assert_eq!(
        out[0].column("reserve").unwrap().lane(0),
        reserve.as_slice()
    );
}

/// Every extra level is another full traversal, so more than three is a lint,
/// not a silent cost (`W0110`).
#[test]
fn w0110_fires_above_three_levels() {
    let mut src = String::from(BACK_CHANNEL);
    // Chain three more back-channels: each `stage_k` is seeded from an npv over
    // the previous one, which is one more level each time.
    let mut prev = "reserve".to_string();
    for k in 0..3 {
        src.push_str(&format!(
            r#"
[[component]]
name = "pv_{prev}_{k}"
kind = "Derived"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "npv({prev}, disc)"

[[component]]
name = "chain_{k}"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
init = "pv_{prev}_{k}"
expr = "chain_{k}[t-1] * 1.01"
"#
        ));
        prev = format!("chain_{k}");
    }

    let m = model(&src);
    let levels = Levels::analyze(&m.plan, &m.tapes);
    assert_eq!(levels.count(), 5, "{}", levels.text(&m.plan));

    let lints = levels.lints(&m.plan);
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].code, "W0110");
    assert!(lints[0].message.contains("5 stage-2 substage levels"));
    assert_eq!(
        lints[0].doc_url,
        "https://predictable.dev/llm/diagnostics/#W0110"
    );

    // Three levels is still quiet.
    let two = model(BACK_CHANNEL);
    assert!(Levels::analyze(&two.plan, &two.tapes)
        .lints(&two.plan)
        .is_empty());

    // And it still computes: the deepest chain is seeded from the level below.
    let out = run_simple(
        &m,
        &[chunk(
            0,
            &[("sum_assured", vec![100_000.0]), ("q", vec![0.01])],
        )],
    );
    let value = out[0].column("chain_2").unwrap().lane(0)[0];
    assert!(value.is_finite() && value > 0.0);
}

/// The engine surfaces the lint too, so a runner does not have to re-derive it.
#[test]
fn the_engine_reports_its_own_levels() {
    let m = model(BACK_CHANNEL);
    let engine =
        Engine::new(&m.plan, &m.tapes, vec![], &m.timeline, RunConfig::default()).expect("engine");
    assert_eq!(engine.levels().count(), 2);
    assert!(engine.level_lints().is_empty());
}
