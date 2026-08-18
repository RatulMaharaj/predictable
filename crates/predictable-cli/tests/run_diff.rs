//! `predictable diff run` end to end, against runs the engine actually produced.
//!
//! The seeded divergence here is a real one: the `term_annual` reference model is run twice, with
//! one assumption changed between the runs, and the diff has to say *which component* the change
//! surfaced in, *at which `t`*, and what merely inherited it. Nothing is mutated after the fact —
//! every number in both runs came out of the kernel.

use std::path::{Path, PathBuf};

use predictable_cli::dispatch;
use serde_json::Value;

fn cli(args: &[&str]) -> (i32, String, String) {
    let argv: Vec<String> = args.iter().map(|a| (*a).to_string()).collect();
    dispatch(&argv)
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels below the repo root")
        .to_path_buf()
}

/// A scratch copy of `models/term_annual`, so the test may edit its assumptions.
fn model_copy(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("predictable-cli-rundiff-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(&repo().join("models/term_annual"), &dir);
    dir
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        // `runs/` holds the committed golden run and `__pycache__` holds nothing useful; the test
        // makes its own runs.
        if name == "runs" || name == "__pycache__" {
            continue;
        }
        let target = to.join(&name);
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn p(dir: &Path, name: &str) -> String {
    dir.join(name).display().to_string()
}

/// Run the reference model, and return the run directory.
fn run_model(dir: &Path, out: &str) -> String {
    let out = p(dir, out);
    let (code, stdout, stderr) = cli(&["run", &p(dir, "run.pir"), "--out", &out, "--json"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    out
}

/// Two runs of the reference model, identical but for `expense_inflation`.
///
/// Each run gets its *own* copy of the model, so both models still exist afterwards and the diff
/// can attribute the divergence to the change. Editing one tree in place would leave side A's
/// recorded `.pir` digest pointing at a file that no longer matches, and the diff would — rightly
/// — refuse to classify from it.
fn two_runs(tag: &str) -> (String, String, PathBuf) {
    let dir_a = model_copy(&format!("{tag}-a"));
    let a = run_model(&dir_a, "run_a");

    let dir_b = model_copy(&format!("{tag}-b"));
    let base = dir_b.join("base.pir");
    let text = std::fs::read_to_string(&base).unwrap();
    assert!(text.contains("expense_inflation = 0.028"));
    std::fs::write(
        &base,
        text.replace("expense_inflation = 0.028", "expense_inflation = 0.038"),
    )
    .unwrap();
    let b = run_model(&dir_b, "run_b");
    (a, b, dir_a)
}

#[test]
fn one_changed_assumption_localises_to_the_component_that_reads_it() {
    let (a, b, _dir) = two_runs("assumption");
    let (code, stdout, stderr) = cli(&["diff", "run", &a, &b, "--json"]);
    assert_eq!(code, 1, "a divergence beyond tolerance is exit 1\n{stderr}");

    let doc: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(doc["format"], "pvf/1");
    assert_eq!(doc["kind"], "rundiff");
    assert_eq!(doc["summary"]["verdict"], "diverged");
    assert_eq!(doc["summary"]["root_divergences"], 1);
    assert_eq!(doc["summary"]["graph_available"], true);
    assert_eq!(doc["summary"]["model_attribution_available"], true);

    // The root is the expense component — the only one that reads `expense_inflation` — and the
    // first year it can show up in is `t = 1`, because `compound(i, 0) = 1` whatever `i` is.
    let root = &doc["findings"][0];
    assert_eq!(root["class"], "root");
    assert_eq!(root["component"], "model.renewal_expenses");
    assert_eq!(root["t_first"], 1);
    assert_eq!(
        doc["summary"]["first_divergence"]["component"],
        "model.renewal_expenses"
    );
    assert_eq!(doc["summary"]["first_divergence"]["t"], 1);

    // An assumption change is a model change, so the divergence is *explained*: the loop should
    // spend its attention on the unexplained ones.
    assert_eq!(root["explained_by_model_change"]["changed"], true);

    // Everything downstream of the expense flow is inherited, and nothing else diverges at all.
    let by_component: Vec<(String, String)> = doc["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["component"].as_str().unwrap().to_string(),
                f["class"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    for expected in [
        "model.net_cashflow",
        "model.pv_expenses",
        "model.bel",
        "model.reserve",
        "model.profit_margin",
    ] {
        assert!(
            by_component.contains(&(expected.to_string(), "inherited".to_string())),
            "{expected} should be inherited: {by_component:?}"
        );
    }
    for untouched in ["model.premium_income", "model.deaths", "model.death_claims"] {
        assert!(
            by_component.iter().all(|(c, _)| c != untouched),
            "{untouched} should not diverge: {by_component:?}"
        );
    }
}

#[test]
fn the_headline_names_the_output_the_component_and_the_period() {
    let (a, b, _dir) = two_runs("headline");
    let (code, stdout, _) = cli(&["diff", "run", &a, &b]);
    assert_eq!(code, 1);
    // §5.2, verbatim shape: "<output> diverges at t=<t> in component <component>: <a> vs <b>".
    assert!(
        stdout.contains("diverges at t=1 in component renewal_expenses:"),
        "{stdout}"
    );
    assert!(stdout.contains("DIVERGED"));
    assert!(stdout.contains("F001  root"));
    assert!(stdout.contains("explained by a model change"));
    // Under prose, no JSON; under --json, no prose. Never both.
    assert!(!stdout.contains("\"format\""));
    let (_, json, _) = cli(&["diff", "run", &a, &b, "--json"]);
    assert!(!json.contains("DIVERGED"));
}

#[test]
fn the_same_run_twice_is_zero_and_says_nothing() {
    let (a, _b, _dir) = two_runs("identical");
    let (code, stdout, _) = cli(&["diff", "run", &a, &a, "--json"]);
    assert_eq!(code, 0);
    let doc: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(doc["summary"]["verdict"], "matched");
    assert_eq!(doc["summary"]["cells"]["diverged"], 0);
    assert!(doc["findings"].as_array().unwrap().is_empty());
    assert!(doc["summary"]["cells"]["compared"].as_u64().unwrap() > 7000);
}

#[test]
fn the_tolerance_profile_decides_and_an_unknown_one_is_a_usage_error() {
    let (a, b, _dir) = two_runs("profiles");

    // `materiality` is looser than `reconcile`, but a 1% inflation change is not a rounding
    // difference and no profile in §5.6 makes it one.
    for profile in ["exact", "regression", "reconcile", "materiality"] {
        let (code, _, _) = cli(&["diff", "run", &a, &b, "--tolerance-profile", profile]);
        assert_eq!(code, 1, "{profile}");
    }

    let (code, _, stderr) = cli(&["diff", "run", &a, &b, "--tolerance-profile", "loose"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("reconcile"), "{stderr}");

    // A custom pair wide enough to swallow the whole portfolio matches.
    let (code, _, _) = cli(&["diff", "run", &a, &b, "--abs", "1e12", "--rel", "1"]);
    assert_eq!(code, 0);
}

#[test]
fn fail_on_narrows_the_exit_code_without_hiding_a_finding() {
    let (a, b, _dir) = two_runs("fail-on");
    let (any, _, _) = cli(&["diff", "run", &a, &b, "--fail-on", "any"]);
    let (root, _, _) = cli(&["diff", "run", &a, &b, "--fail-on", "root"]);
    let (never, stdout, _) = cli(&["diff", "run", &a, &b, "--fail-on", "never", "--json"]);
    assert_eq!((any, root, never), (1, 1, 0));
    // Even at `--fail-on never`, the findings are all still there.
    let doc: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(doc["summary"]["verdict"], "diverged");
    assert!(!doc["findings"].as_array().unwrap().is_empty());

    let (code, _, stderr) = cli(&["diff", "run", &a, &b, "--fail-on", "sometimes"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("any | root | never"), "{stderr}");
}

#[test]
fn filters_and_top_narrow_the_report() {
    let (a, b, _dir) = two_runs("filters");
    let (_, stdout, _) = cli(&[
        "diff",
        "run",
        &a,
        &b,
        "--component",
        "renewal_expenses",
        "--json",
    ]);
    let doc: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(doc["findings"].as_array().unwrap().len(), 1);
    assert_eq!(doc["findings"][0]["component"], "model.renewal_expenses");

    let (_, stdout, _) = cli(&["diff", "run", &a, &b, "--top", "2", "--json"]);
    let doc: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(doc["findings"].as_array().unwrap().len(), 2);
    assert_eq!(doc["findings_truncated"]["kept"], 2);
    assert_eq!(doc["findings"][0]["component"], "model.renewal_expenses");

    let (_, stdout, _) = cli(&["diff", "run", &a, &b, "--mp", "TA00005", "--json"]);
    let doc: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(doc["summary"]["modelpoints"]["common"], 25);
    assert_eq!(doc["findings"][0]["n_modelpoints"], 1);
    assert_eq!(doc["findings"][0]["exemplar"]["mp_key"], "TA00005");
}

#[test]
fn explain_tolerance_prints_the_rule_that_matched_each_component() {
    let (a, b, _dir) = two_runs("explain-tolerance");
    let (_, stdout, _) = cli(&["diff", "run", &a, &b, "--explain-tolerance"]);
    assert!(stdout.contains("tolerance by component"), "{stdout}");
    assert!(stdout.contains("model.renewal_expenses"));
    assert!(stdout.contains("by_unit[money]"));
}

#[test]
fn out_json_writes_the_same_document_the_agent_reads_from_stdout() {
    let (a, b, dir) = two_runs("out-json");
    let out = p(&dir, "diff.json");
    let (code, stdout, _) = cli(&["diff", "run", &a, &b, "--json", "--out-json", &out]);
    assert_eq!(code, 1);
    let file: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    let piped: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(file, piped);
    assert_eq!(file["kind"], "rundiff");
}

#[test]
fn a_missing_run_directory_is_a_usage_failure_not_a_divergence() {
    let (a, _b, dir) = two_runs("missing");
    let (code, _, stderr) = cli(&["diff", "run", &a, &p(&dir, "nope")]);
    assert_eq!(code, 2);
    assert!(stderr.contains("not a run directory"), "{stderr}");

    let (code, _, stderr) = cli(&["diff", "run", &a]);
    assert_eq!(code, 2);
    assert!(stderr.contains("two run directories"), "{stderr}");
}

#[test]
fn diff_still_needs_its_subject_word() {
    let (a, b, _dir) = two_runs("subject");
    // `diff A B` without `run` or `model` is refused rather than guessed at: the same command
    // must not sometimes compare formulas and sometimes compare numbers.
    let (code, _, _) = cli(&["diff", &a, &b]);
    assert_eq!(code, 2);
}

// ---------------------------------------------------------------------------
// a Prophet run is a run
// ---------------------------------------------------------------------------

/// A `.rpt` carrying run A's own numbers, written the way Prophet writes them: 1-based periods,
/// two decimal places, and one column with no predictable counterpart.
fn rpt_of(run: &str, expense_factor: f64) -> String {
    let side = predictable_rundiff::RunSide::load("a", run).unwrap();
    let at = |id: &str, mp: u32, t: i32| -> f64 {
        side.components
            .get(id)
            .and_then(|c| c.get(&(mp, t)))
            .and_then(predictable_rundiff::Value::as_f64)
            .unwrap_or(0.0)
    };
    let mut text = String::from(
        "! Prophet results\n\
         RUN, TERM_BASE_2026Q2\n\
         PRODUCT, TERM_UK\n\
         RUN_DATE, 30/06/2026\n\
         TIME_UNITS, YEARS\n\
         NUM_PERIODS, 40\n\
         POL_NUM, PERIOD, DTH_CLAIM, EXPENSE, RESERVE_INT\n",
    );
    for (mp_row, mp_key) in &side.mp_keys {
        for t in 0..=40 {
            text.push_str(&format!(
                "{mp_key}, {}, {:.2}, {:.2}, 0.00\n",
                t + 1,
                at("model.death_claims", *mp_row, t),
                at("model.renewal_expenses", *mp_row, t) * expense_factor,
            ));
        }
    }
    text
}

const MAPPING: &str = r#"
format = "pvf/1"
model_module = "model"
period_base = 1
mp_key = { prophet = "POL_NUM", predictable = "policy_number" }

[[component]]
prophet = "EXPENSE"
predictable = "renewal_expenses"

[[component]]
prophet = "DTH_CLAIM"
predictable = "death_claims"

[[unmapped]]
prophet = "RESERVE_INT"
reason = "intermediate; no predictable equivalent"
"#;

#[test]
fn a_prophet_rpt_is_diffed_by_importing_it_into_a_run_directory_first() {
    let (a, _b, dir) = two_runs("rpt");
    let mapping = p(&dir, "mapping.toml");
    std::fs::write(&mapping, MAPPING).unwrap();

    // A faithful export of run A reconciles to run A: the 1-based periods are rebased at import,
    // and the file's own 2dp precision raises the tolerance for the columns it wrote.
    let faithful = p(&dir, "faithful.rpt");
    std::fs::write(&faithful, rpt_of(&a, 1.0)).unwrap();
    let (code, stdout, stderr) = cli(&[
        "diff",
        "run",
        &faithful,
        &a,
        "--mapping",
        &mapping,
        "--json",
    ]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let doc: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(doc["a"]["system"], "prophet");
    assert_eq!(doc["b"]["system"], "predictable");
    assert_eq!(doc["summary"]["verdict"], "matched");
    assert_eq!(doc["summary"]["components"]["common"], 2);
    assert_eq!(
        doc["structural"]["only_in_a"][0]["component"],
        "prophet.reserve_int"
    );
    assert_eq!(
        doc["structural"]["only_in_a"][0]["reason"],
        "declared unmapped"
    );

    // The same export with an inflated expense column diverges, and localises to the component
    // the mapping says that column is.
    let inflated = p(&dir, "inflated.rpt");
    std::fs::write(&inflated, rpt_of(&a, 1.05)).unwrap();
    let (code, stdout, _) = cli(&[
        "diff",
        "run",
        &inflated,
        &a,
        "--mapping",
        &mapping,
        "--json",
    ]);
    assert_eq!(code, 1);
    let doc: Value = serde_json::from_str(&stdout).unwrap();
    let f = &doc["findings"][0];
    assert_eq!(f["component"], "model.renewal_expenses");
    assert_eq!(f["source_component"], "prophet.expense");
    assert_eq!(f["class"], "root");
    // No Prophet model exists to diff, so attribution is `null` rather than "unexplained".
    assert_eq!(doc["summary"]["model_attribution_available"], false);
    assert!(f["explained_by_model_change"].is_null());
}

#[test]
fn a_model_edited_since_the_run_is_not_that_runs_model() {
    let (a, b, dir) = two_runs("drift");
    // Touch side A's model after the fact. Its manifest's digest no longer matches, so the diff
    // must not classify from it — and must say that it could not, rather than quietly guessing.
    let model = dir.join("build/model.pir");
    let text = std::fs::read_to_string(&model).unwrap();
    std::fs::write(&model, format!("{text}\n# edited after the run\n")).unwrap();

    let (_, stdout, _) = cli(&["diff", "run", &a, &b, "--json"]);
    let doc: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(doc["summary"]["model_attribution_available"], false);
    // `b`'s model is still intact, so the classification is still the IR's.
    assert_eq!(doc["summary"]["graph_available"], true);
    assert_eq!(doc["findings"][0]["class"], "root");
    assert!(doc["findings"][0]["explained_by_model_change"].is_null());
}
