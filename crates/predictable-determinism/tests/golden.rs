//! The golden corpus (`03-engine.md` §7 clause 7).
//!
//! Every valid conformance case that can execute is projected and rendered byte for byte into
//! `golden/`. A changed golden is a changed result, and has to be regenerated in the same commit
//! as the change that caused it — the same reviewability principle as `.pir` diffability.

use predictable_determinism::corpus::{self, Executors};
use predictable_determinism::{assert_golden, cases, load, project, render_golden};

/// Every valid case either produces its golden, or is recorded as a skip with a reason.
#[test]
fn the_golden_corpus_matches_byte_for_byte() {
    let mut skips = String::from("# cases the harness cannot execute, and why\n");
    let mut failures = Vec::new();
    let mut ran = 0;

    for case in cases() {
        match load(&case) {
            Err(why) => skips.push_str(&format!("{} {}\n", case.name, why)),
            Ok(loaded) => match project(&loaded, 1024, &predictable_runner::SerialExecutor) {
                Err(why) => skips.push_str(&format!("{} {}\n", case.name, why)),
                Ok(out) => {
                    ran += 1;
                    let text = render_golden(&loaded, &out);
                    if let Err(e) = assert_golden(&format!("{}.golden", case.name), &text) {
                        failures.push(e);
                    }
                }
            },
        }
    }

    // The skip list is itself a golden: a case that silently stops executing is the failure mode
    // a determinism corpus is least able to notice.
    if let Err(e) = assert_golden("_skipped.txt", &skips) {
        failures.push(e);
    }
    assert!(ran >= 4, "only {ran} corpus cases executed");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The same case, projected twice, is the same bytes — the floor every other claim stands on.
#[test]
fn a_run_repeated_is_byte_identical() {
    for case in cases() {
        let Ok(loaded) = load(&case) else { continue };
        let Ok(a) = project(&loaded, 1024, &predictable_runner::SerialExecutor) else {
            continue;
        };
        let b = project(&loaded, 1024, &predictable_runner::SerialExecutor).unwrap();
        assert_eq!(
            render_golden(&loaded, &a),
            render_golden(&loaded, &b),
            "{} is not reproducible against itself",
            case.name
        );
    }
}

/// The digest chain is a function of the model, not of how it was run (§7 clause 6).
#[test]
fn digests_do_not_depend_on_chunking_or_executor() {
    for case in cases() {
        let Ok(loaded) = load(&case) else { continue };
        let Ok(base) = project(&loaded, 1, &predictable_runner::SerialExecutor) else {
            continue;
        };
        let head = |text: String| {
            text.lines()
                .filter(|l| {
                    l.starts_with("model_digest")
                        || l.starts_with("order_digest")
                        || l.starts_with("plan_digest")
                        || l.starts_with("tape_digest")
                })
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        let want = head(render_golden(&loaded, &base));
        assert_eq!(want.len(), 4, "{}: incomplete digest chain", case.name);
        for (label, exec) in Executors::all() {
            for chunk in [1usize, 3, 1024] {
                let out = project(&loaded, chunk, exec.as_ref()).unwrap();
                assert_eq!(
                    want,
                    head(render_golden(&loaded, &out)),
                    "{}: digests moved under {label} C={chunk}",
                    case.name
                );
            }
        }
    }
}

/// The synthetic modelpoint file is a pure function of the schema.
#[test]
fn synthetic_modelpoints_are_reproducible() {
    for case in cases() {
        let Ok(loaded) = load(&case) else { continue };
        let again = load(&case).unwrap();
        assert_eq!(
            loaded.modelpoints, again.modelpoints,
            "{} modelpoints are not a pure function of the case",
            case.name
        );
        assert_eq!(
            corpus::SYNTHETIC_ROWS,
            4,
            "the golden row count is part of the corpus contract"
        );
    }
}
