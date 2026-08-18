//! Behavioural tests for the chunk pipeline: executor equivalence, chunk-size invariance,
//! emission modes, the trap policies of `01-ir.md` §9.3.1 and the cancel flag of
//! `03-engine.md` §10.

mod common;

use common::{chunk, model, nums, runner, term_chunks, TERM, TRAPPING};
use predictable_io::outbound::Outcome;
use predictable_ir::run::{Emit, OnTrap};
use predictable_runner::{minimal_run_file, CancelFlag, LocalExecutor, SerialExecutor};

#[test]
fn the_executor_is_a_scheduling_choice_not_a_numerical_one() {
    let m = model(TERM);
    let run = minimal_run_file();
    let r = runner(&m, &run);
    let chunks = term_chunks(2);
    let cancel = CancelFlag::new();

    let serial = r.project(&chunks, &SerialExecutor, &cancel).unwrap();
    let parallel = r.project(&chunks, &LocalExecutor::new(4), &cancel).unwrap();

    assert_eq!(serial.chunks.len(), parallel.chunks.len());
    for (a, b) in serial.chunks.iter().zip(&parallel.chunks) {
        // Chunk index order, not completion order.
        assert_eq!(a.index, b.index);
        assert_eq!(a.keys, b.keys);
        for (ca, cb) in a.columns.iter().zip(&b.columns) {
            assert_eq!(ca.name, cb.name);
            // Bit equality, not approximate equality.
            let lhs: Vec<u64> = ca.values.iter().map(|v| v.to_bits()).collect();
            let rhs: Vec<u64> = cb.values.iter().map(|v| v.to_bits()).collect();
            assert_eq!(lhs, rhs, "{} differs between executors", ca.name);
        }
    }
}

#[test]
fn chunk_size_does_not_change_a_number() {
    let m = model(TERM);
    let run = minimal_run_file();
    let r = runner(&m, &run);
    let cancel = CancelFlag::new();

    let one = r
        .project(&term_chunks(1), &SerialExecutor, &cancel)
        .unwrap();
    let many = r
        .project(&term_chunks(1024), &LocalExecutor::new(2), &cancel)
        .unwrap();

    let flat = |p: &predictable_runner::Projection| -> Vec<(String, u64)> {
        let mut out = Vec::new();
        for c in &p.chunks {
            for (lane, key) in c.keys.iter().enumerate() {
                let bel = c.column("term.bel").unwrap().lane(lane)[0];
                out.push((key.clone(), bel.to_bits()));
            }
        }
        out
    };
    assert_eq!(flat(&one), flat(&many));
    assert_eq!(one.modelpoints_projected, 4);
    assert_eq!(many.modelpoints_projected, 4);
}

#[test]
fn the_kernels_numbers_are_the_ones_that_come_out() {
    let m = model(TERM);
    let run = minimal_run_file();
    let r = runner(&m, &run);
    let p = r
        .project(&term_chunks(4), &SerialExecutor, &CancelFlag::new())
        .unwrap();

    // POL1: q = 0.01, sum_assured = 100_000, premium = 900, T = 3.
    let claims = p.chunks[0].column("term.claims").unwrap().lane(0).to_vec();
    let mut survivors = 1.0;
    let mut expected = Vec::new();
    for _ in 0..=3 {
        expected.push(survivors * 0.01 * 100_000.0);
        survivors *= 1.0 - 0.01;
    }
    assert_eq!(claims, expected);

    // `bel` is a stage-2 reduction, summed left to right in `t`.
    let mut sum = 0.0;
    let net = p.chunks[0]
        .column("term.net_cashflow")
        .unwrap()
        .lane(0)
        .to_vec();
    for v in &net {
        sum += *v;
    }
    assert_eq!(p.chunks[0].column("term.bel").unwrap().lane(0)[0], sum);
}

#[test]
fn emit_all_adds_the_derived_components_and_emit_list_adds_named_ones() {
    let m = model(TERM);

    let mut run = minimal_run_file();
    let outputs = runner(&m, &run).emitted().len();

    run.run.emit = Emit::All;
    let all = runner(&m, &run).emitted().len();
    assert!(all > outputs, "emit = all is the migration mode");
    assert!(runner(&m, &run)
        .emitted()
        .iter()
        .any(|c| c.id == "term.survivors"));

    let mut listed = minimal_run_file();
    listed.run.emit = Emit::List;
    listed.run.emit_list = vec!["survivors".to_string()];
    let r = runner(&m, &listed);
    // `emit` never removes an Output (§8.4.3).
    assert!(r.emitted().iter().any(|c| c.id == "term.claims"));
    assert!(r.emitted().iter().any(|c| c.id == "term.survivors"));
    assert_eq!(r.emitted().len(), outputs + 1);
}

#[test]
fn emit_list_naming_nothing_is_refused() {
    let m = model(TERM);
    let mut run = minimal_run_file();
    run.run.emit = Emit::List;
    run.run.emit_list = vec!["not_a_component".to_string()];
    let err = predictable_runner::Runner::new(predictable_runner::RunInputs {
        modules: &m.modules,
        plan: &m.plan,
        tapes: &m.tapes,
        tables: vec![],
        assumptions: Default::default(),
        run: &run,
    })
    .unwrap_err();
    assert!(err.to_string().contains("E0106"), "{err}");
}

#[test]
fn a_trap_under_abort_stops_the_run_and_writes_no_results() {
    let m = model(TRAPPING);
    let run = minimal_run_file(); // on_trap = "abort" is the default.
    let r = runner(&m, &run);
    let chunks = vec![chunk(
        0,
        0,
        &["POL1", "POL2"],
        &[
            ("numerator", nums(&[1.0, 2.0])),
            ("exposure", nums(&[1.0, 0.0])),
        ],
    )];

    let p = r
        .project(&chunks, &SerialExecutor, &CancelFlag::new())
        .unwrap();
    assert_eq!(p.outcome, Outcome::Aborted);
    assert_eq!(p.exit_code(), 2);
    assert!(p.chunks.is_empty(), "no partial results under abort");
    assert_eq!(p.traps.len(), 1);
    let envelope = p.traps[0].envelope();
    assert_eq!(envelope["code"], "E0902");
    assert_eq!(envelope["trap"], "div_by_zero");
    assert_eq!(envelope["mp_key"], "POL2");
}

#[test]
fn a_trap_under_continue_drops_the_modelpoint_entirely() {
    let m = model(TRAPPING);
    let mut run = minimal_run_file();
    run.run.on_trap = OnTrap::Continue;
    let r = runner(&m, &run);
    let chunks = vec![chunk(
        0,
        0,
        &["POL1", "POL2", "POL3"],
        &[
            ("numerator", nums(&[1.0, 2.0, 3.0])),
            ("exposure", nums(&[1.0, 0.0, 4.0])),
        ],
    )];

    let p = r
        .project(&chunks, &SerialExecutor, &CancelFlag::new())
        .unwrap();
    assert_eq!(p.outcome, Outcome::CompletedWithTraps);
    assert_eq!(p.exit_code(), 1);
    assert_eq!(p.modelpoints_projected, 2);
    assert_eq!(p.modelpoints_trapped, 1);
    // No null rows: the modelpoint is simply not there (§2.11, §9.3.1).
    assert_eq!(
        p.chunks[0].keys,
        vec!["POL1".to_string(), "POL3".to_string()]
    );
}

#[test]
fn max_errors_caps_reports_but_not_counts() {
    let m = model(TRAPPING);
    let mut run = minimal_run_file();
    run.run.on_trap = OnTrap::Continue;
    run.run.max_errors = 1;
    let r = runner(&m, &run);
    let chunks = vec![chunk(
        0,
        0,
        &["POL1", "POL2", "POL3"],
        &[
            ("numerator", nums(&[1.0, 2.0, 3.0])),
            ("exposure", nums(&[0.0, 0.0, 0.0])),
        ],
    )];

    let p = r
        .project(&chunks, &SerialExecutor, &CancelFlag::new())
        .unwrap();
    assert_eq!(p.traps.len(), 1, "reports are capped");
    assert_eq!(p.modelpoints_trapped, 3, "counts stay exact");
}

#[test]
fn a_cancelled_run_is_short_and_says_so() {
    let m = model(TERM);
    let run = minimal_run_file();
    let r = runner(&m, &run);
    let cancel = CancelFlag::new();
    cancel.cancel();

    let p = r
        .project(&term_chunks(2), &SerialExecutor, &cancel)
        .unwrap();
    assert_eq!(p.outcome, Outcome::Cancelled);
    assert_eq!(p.exit_code(), 1);
    assert_eq!(p.modelpoints_projected, 0);
    assert!(p.chunks.is_empty());
}

#[test]
fn cancelling_after_the_first_chunk_keeps_what_completed() {
    let m = model(TERM);
    let run = minimal_run_file();
    let r = runner(&m, &run);
    let chunks = term_chunks(1);
    let cancel = CancelFlag::new();

    // Serial execution: cancel from a worker-visible flag after chunk 0 has run.
    let flag = cancel.clone();
    let executor = SerialExecutor;
    // Project chunk 0 alone, then cancel and project the rest — the same observable sequence a
    // Ctrl-C mid-run produces, without a race in the test.
    let first = r.project(&chunks[..1], &executor, &cancel).unwrap();
    assert_eq!(first.modelpoints_projected, 1);
    flag.cancel();
    let rest = r.project(&chunks[1..], &executor, &cancel).unwrap();
    assert_eq!(rest.outcome, Outcome::Cancelled);
    assert_eq!(rest.modelpoints_projected, 0);
}
