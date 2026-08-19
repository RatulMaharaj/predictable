//! Lowering against the conformance corpus (T07).
//!
//! Every valid case must lower, and the tape it lowers to must satisfy the
//! invariants the kernel is allowed to assume. The corpus was written from the
//! spec before any of this existed, so a case that stops lowering is a
//! regression against `01-ir.md`, not against a golden file.

use std::path::{Path, PathBuf};

use predictable_check::Input;
use predictable_plan::{plan_sources, OptLevel, PlanOptions};
use predictable_tape::{lower_plan, Op, Tape, TapeProgram};

fn cases() -> Vec<(String, Vec<Input>)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/valid")
        .canonicalize()
        .expect("the conformance corpus is checked in");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs.into_iter()
        .map(|d| {
            let mut files: Vec<PathBuf> = std::fs::read_dir(&d)
                .unwrap()
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|e| e == "pir"))
                .collect();
            files.sort();
            let inputs = files
                .into_iter()
                .map(|p| {
                    let text = std::fs::read_to_string(&p).unwrap();
                    Input::new(p.file_name().unwrap().to_string_lossy().to_string(), text)
                })
                .collect();
            (d.file_name().unwrap().to_string_lossy().to_string(), inputs)
        })
        .collect()
}

fn lower(inputs: &[Input], opt: OptLevel) -> TapeProgram {
    let plan = plan_sources(
        inputs,
        &PlanOptions {
            opt,
            ..Default::default()
        },
    )
    .expect("plans");
    lower_plan(&plan).expect("lowers")
}

/// The invariants a kernel may rely on without checking.
fn check_tape(case: &str, label: &str, tape: &Tape) {
    for (i, op) in tape.ops.iter().enumerate() {
        op.for_each_reg(|r, _| {
            assert!(
                r.0 < tape.n_regs,
                "{case}/{label} op {i}: {r} outside a {}-register frame",
                tape.n_regs
            );
        });
        if let Some(site) = op.site() {
            assert!(
                (site.0 as usize) < tape.sites.len(),
                "{case}/{label} op {i}: dangling site"
            );
        }
        match op {
            Op::ConstF(_, c) => assert!((c.0 as usize) < tape.const_f.len()),
            Op::ConstI(_, c) => assert!((c.0 as usize) < tape.const_i.len()),
            Op::ConstS(_, c) => assert!((c.0 as usize) < tape.const_s.len()),
            Op::Cum(_, _, acc) => assert!(acc.0 < tape.accumulators),
            _ => {}
        }
        // Only trapping ops may be masked, and only stage 2 may reduce.
        if op.mask().is_some() {
            assert!(
                op.traps(),
                "{case}/{label} op {i}: mask on a non-trapping op"
            );
        }
    }
    // Every range ends in a store: a range that computes nothing storable would
    // be dead code the kernel still pays for.
    for r in &tape.ranges {
        assert!(
            matches!(
                tape.ops[r.end as usize - 1],
                Op::StoreCur(..)
                    | Op::StoreSeed(..)
                    | Op::StoreHoisted(..)
                    | Op::StorePerMp(..)
                    | Op::StoreScalar(..)
            ),
            "{case}/{label}: range for {} does not end in a store",
            r.slot
        );
    }
}

#[test]
fn every_valid_case_lowers() {
    let cases = cases();
    assert!(cases.len() >= 8, "found only {} cases", cases.len());
    for (name, inputs) in cases {
        let prog = lower(&inputs, OptLevel::O1);
        assert!(prog.op_count() > 0, "{name} lowered to nothing");
        for (label, tape) in prog.tapes() {
            check_tape(&name, &label, tape);
        }
    }
}

#[test]
fn lowering_is_reproducible_across_the_whole_corpus() {
    for (name, inputs) in cases() {
        let a = lower(&inputs, OptLevel::O1);
        let b = lower(&inputs, OptLevel::O1);
        assert_eq!(a.digest, b.digest, "{name}");
        assert_eq!(a, b, "{name}");
    }
}

#[test]
fn o0_lowers_the_same_model_without_hoisting() {
    // `--O0` turns off the planner's optional passes. Lowering must still
    // produce a runnable program — that is what makes `--O0` usable for
    // bisecting a bit-level difference (§3.5).
    for (name, inputs) in cases() {
        let o0 = lower(&inputs, OptLevel::O0);
        assert!(
            o0.hoisted.body.is_empty(),
            "{name}: nothing is hoisted at O0"
        );
        assert!(o0.op_count() > 0, "{name}");
        for (label, tape) in o0.tapes() {
            check_tape(&name, &label, tape);
        }
    }
}

#[test]
fn the_peel_covers_every_lag_in_the_corpus() {
    for (name, inputs) in cases() {
        let plan = plan_sources(&inputs, &PlanOptions::default()).unwrap();
        let prog = lower_plan(&plan).unwrap();
        let max_lag = plan.series.iter().map(|s| s.max_lag).max().unwrap_or(0);
        assert_eq!(prog.peel, max_lag.min(plan.periods), "{name}");
        assert_eq!(
            prog.stage1.prefix.len(),
            prog.peel.saturating_sub(1) as usize
        );
        // No tape at or past the peel may contain a pre-origin read.
        assert!(
            !prog
                .stage1
                .body
                .ops
                .iter()
                .any(|o| matches!(o, Op::LoadSeed(..))),
            "{name}: the body tape still reads a seed"
        );
    }
}
