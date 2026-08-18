//! The planner against the conformance corpus (T07).
//!
//! Every valid case in `conformance/valid/` must plan, and must plan the same
//! way twice. The corpus was authored from the spec before any of this existed,
//! so it is a specification test rather than a snapshot of the planner's
//! behaviour: a case that stops planning is a regression against `01-ir.md`,
//! not against a golden file someone regenerated.

use std::path::{Path, PathBuf};

use predictable_check::Input;
use predictable_plan::{plan_sources, OptLevel, PlanOptions};

fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/valid")
        .canonicalize()
        .expect("the conformance corpus is checked in")
}

/// Every `.pir` in a case directory, in sorted order so the module order — and
/// therefore the `E0204` "already defined" attribution — is stable.
fn case_inputs(dir: &Path) -> Vec<Input> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "pir"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let text = std::fs::read_to_string(&p).unwrap();
            Input::new(p.file_name().unwrap().to_string_lossy().to_string(), text)
        })
        .collect()
}

fn cases() -> Vec<(String, Vec<Input>)> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(corpus_root())
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs.into_iter()
        .map(|d| {
            (
                d.file_name().unwrap().to_string_lossy().to_string(),
                case_inputs(&d),
            )
        })
        .collect()
}

#[test]
fn every_valid_case_plans() {
    let cases = cases();
    assert!(cases.len() >= 8, "found only {} cases", cases.len());
    for (name, inputs) in cases {
        let plan = match plan_sources(&inputs, &PlanOptions::default()) {
            Ok(p) => p,
            Err(e) => panic!("{name} did not plan: {e}"),
        };
        // A plan that names nothing is a plan that silently dropped the model.
        assert!(!plan.slot_refs.is_empty(), "{name} allocated no slots");
        // The order is total over every slot.
        assert_eq!(plan.order().len(), plan.slot_refs.len(), "{name}");
        // Timeline aside, a valid corpus model has outputs to emit.
        assert!(!plan.outputs.is_empty(), "{name} emits nothing");
    }
}

#[test]
fn planning_is_reproducible_across_the_whole_corpus() {
    for (name, inputs) in cases() {
        let a = plan_sources(&inputs, &PlanOptions::default()).unwrap();
        let b = plan_sources(&inputs, &PlanOptions::default()).unwrap();
        assert_eq!(a.order_digest, b.order_digest, "{name}");
        assert_eq!(a, b, "{name}");
    }
}

#[test]
fn o0_and_o1_agree_on_order_membership_across_the_corpus() {
    // `--O0` moves work between tapes but never invents or loses a slot: the
    // *set* of ordered slots is invariant under optimisation, which is what
    // makes `--O0` a usable bisection tool rather than a different model.
    for (name, inputs) in cases() {
        let o1 = plan_sources(&inputs, &PlanOptions::default()).unwrap();
        let o0 = plan_sources(
            &inputs,
            &PlanOptions {
                opt: OptLevel::O0,
                ..PlanOptions::default()
            },
        )
        .unwrap();
        let mut a = o1.order();
        let mut b = o0.order();
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b, "{name}");
        assert_eq!(o1.outputs, o0.outputs, "{name}");
        // Retention is a semantic decision, not an optimisation: it must not
        // move with `--O0`.
        for (x, y) in o1.series.iter().zip(&o0.series) {
            assert_eq!(x.retention, y.retention, "{name}: {}", x.info.name);
            assert_eq!(x.max_lag, y.max_lag, "{name}: {}", x.info.name);
        }
    }
}

#[test]
fn the_term_assurance_reference_model_hoists_its_discounting() {
    // The worked example of `01-ir.md` §6. Its yield-curve-shaped series must
    // leave the per-modelpoint loop (§3.4), and its per-policy series must not.
    let dir = corpus_root().join("v02-term-assurance");
    let plan = plan_sources(&case_inputs(&dir), &PlanOptions::default()).unwrap();
    assert!(
        !plan.hoisted.is_empty(),
        "nothing hoisted out of the reference model"
    );
    for id in &plan.hoisted {
        let slot = plan.series_of(*id).unwrap();
        assert!(
            slot.hoistable,
            "{} in hoisted but not hoistable",
            slot.info.name
        );
    }
    // Hoisting pays for itself: the per-chunk footprint drops.
    let o0 = plan_sources(
        &case_inputs(&dir),
        &PlanOptions {
            opt: OptLevel::O0,
            ..PlanOptions::default()
        },
    )
    .unwrap();
    assert!(plan.series_bytes_per_chunk(1024) < o0.series_bytes_per_chunk(1024));
}
