//! The seeded-mutation harness of `04-verify.md` §9.2, driven end to end.
//!
//! Nothing here is simulated. Every mutation is a literal edit to a copy of a committed reference
//! model; both sides are run by the same CLI a user runs, through `predictable_cli::dispatch`; and
//! the diff under test is the one `predictable diff run` would print. The assertion is §9.2's:
//! the correct component is reported as the *root* divergence, at the correct `t_first`, with the
//! intended hypothesis code fired.
//!
//! The published number — the root-cause hit rate — is regenerated from this same code by
//! `examples/mutation_report.rs`, so the docs table cannot drift from the tests.

use std::path::{Path, PathBuf};

use predictable_rundiff::mutation::{self, HitRate, Score};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels below the repo root")
        .to_path_buf()
}

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("predictable-mutation-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Run one model with the real CLI.
fn cli_run(run_file: &Path, out: &Path) -> Result<(), String> {
    let argv: Vec<String> = [
        "run",
        &run_file.display().to_string(),
        "--out",
        &out.display().to_string(),
        "--json",
        // `emit = "all"` needs every series retained; the flag is how the planner is told.
        "--retain-all",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let (code, stdout, stderr) = predictable_cli::dispatch(&argv);
    if code != 0 {
        return Err(format!("run exited {code}: {stdout}{stderr}"));
    }
    Ok(())
}

/// The whole catalogue, scored once. Running the models is the expensive part, so the suite pays
/// for it once and asserts against the result many times.
fn scored() -> &'static HitRate {
    use std::sync::OnceLock;
    static ONCE: OnceLock<HitRate> = OnceLock::new();
    ONCE.get_or_init(|| {
        mutation::run_catalogue(&repo().join("models"), &scratch(), &cli_run)
            .expect("the catalogue applies to the committed reference models")
    })
}

fn score(id: &str) -> &'static Score {
    scored()
        .scores
        .iter()
        .find(|s| s.id == id)
        .unwrap_or_else(|| panic!("{id} is in the catalogue"))
}

#[test]
fn every_mutation_actually_moves_the_numbers() {
    // A mutation that changes nothing would score as a differ success for the wrong reason.
    for s in &scored().scores {
        assert!(s.diverged, "{} produced no divergence at all", s.id);
    }
}

#[test]
fn the_root_cause_hit_rate_is_published_and_does_not_regress() {
    let r = scored();
    let failures: Vec<String> = r
        .scores
        .iter()
        .filter(|s| !s.root_cause())
        .map(|s| {
            format!(
                "{}: component {}, t_first {} — roots reported: {}",
                s.id,
                s.component,
                s.t_first,
                s.reported.join(", ")
            )
        })
        .collect();
    // §9.2 makes this the project's primary quality metric, so it is pinned rather than sampled:
    // a regression in localisation has to fail a test, not merely make a table look worse.
    assert_eq!(
        r.root_cause_hits(),
        r.total(),
        "root-cause hit rate {}/{}\n{}",
        r.root_cause_hits(),
        r.total(),
        failures.join("\n")
    );
}

#[test]
fn the_intended_hypothesis_fires_wherever_the_catalogue_names_one() {
    for s in &scored().scores {
        if let Some(hit) = s.hypothesis {
            assert!(
                hit,
                "{}: expected hypothesis did not fire; codes that did: {:?}",
                s.id, s.codes
            );
        }
    }
}

#[test]
fn a_dropped_lag_is_localised_at_the_first_period_it_can_bite() {
    // `num_pols_if[t-1] * (1 - qx[t-1])` became `(1 - qx)`: `t = 0` is seeded from `init` and is
    // therefore identical on both sides. The differ has to say `t = 1`, not `t = 0`.
    let s = score("TA-lag-dropped");
    assert!(s.component && s.t_first, "{s:?}");
}

#[test]
fn the_prophet_twelfth_idiom_is_named_as_such_and_not_merely_as_a_ratio() {
    let s = score("TM-prophet-twelfth-rate");
    assert!(s.codes.contains(&"H0401".to_string()), "{s:?}");
    // H0101 stands down when a specific cause explains the same ratio.
    assert!(!s.codes.contains(&"H0101".to_string()), "{s:?}");
}

#[test]
fn a_rounding_step_is_a_tolerance_finding_not_a_model_change() {
    let s = score("TA-rounded-premium");
    assert!(s.codes.contains(&"H0402".to_string()), "{s:?}");
}

#[test]
fn the_markdown_table_matches_the_scores_it_was_built_from() {
    let md = scored().markdown();
    for s in &scored().scores {
        assert!(md.contains(s.id), "{} missing from the table", s.id);
    }
    assert!(md.contains("Root-cause hit rate:"));
}
