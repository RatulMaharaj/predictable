//! `predictable diff model A B` (`01-ir.md` §11.3) end to end.
//!
//! The contract asserted here is the CLI's, not the diff engine's: the subject
//! word is required, the `pvf/1` document carries the whole answer, a difference
//! is a *result* rather than a failure, and `--fail-on-change` is the only way it
//! becomes one.

use std::fs;
use std::path::{Path, PathBuf};

use predictable_cli::dispatch;
use serde_json::Value;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn cli(args: &[&str]) -> (i32, String, String) {
    let argv: Vec<String> = args.iter().map(|a| (*a).to_string()).collect();
    dispatch(&argv)
}

/// Two copies of `term.pir` in their own directories, `b` optionally edited.
fn pair(tag: &str, edit: &[(&str, &str)]) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("predictable-cli-diff-{tag}"));
    let _ = fs::remove_dir_all(&root);
    let (a, b) = (root.join("a"), root.join("b"));
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();
    let text = fs::read_to_string(Path::new(FIXTURES).join("term.pir")).unwrap();
    fs::write(a.join("term.pir"), &text).unwrap();
    let mut edited = text.clone();
    for (from, to) in edit {
        assert!(edited.contains(from), "fixture has no `{from}`");
        edited = edited.replace(from, to);
    }
    fs::write(b.join("term.pir"), edited).unwrap();
    (a, b)
}

fn json_of(stdout: &str) -> Value {
    serde_json::from_str(stdout).unwrap_or_else(|e| panic!("not JSON: {e}\n{stdout}"))
}

#[test]
fn identical_models_diff_clean_and_exit_zero() {
    let (a, b) = pair("identical", &[]);
    let (code, stdout, _) = cli(&[
        "diff",
        "model",
        &a.display().to_string(),
        &b.display().to_string(),
    ]);
    assert_eq!(code, 0);
    assert!(stdout.contains("no structural differences"), "{stdout}");

    // Even under --fail-on-change, because nothing changed.
    let (code, _, _) = cli(&[
        "diff",
        "model",
        &a.display().to_string(),
        &b.display().to_string(),
        "--fail-on-change",
    ]);
    assert_eq!(code, 0);
}

#[test]
fn a_formula_change_is_a_pvf_document_naming_the_node_and_the_impact_set() {
    // `claims = survivors * q * sum_assured` → the mortality factor is dropped.
    let (a, b) = pair(
        "formula",
        &[(
            "expr = \"survivors * q * sum_assured\"",
            "expr = \"survivors * sum_assured\"",
        )],
    );
    let (code, stdout, _) = cli(&[
        "diff",
        "model",
        &a.display().to_string(),
        &b.display().to_string(),
        "--json",
    ]);
    assert_eq!(code, 0, "a difference is a result, not a failure");

    let doc = json_of(&stdout);
    assert_eq!(doc["format"], "pvf/1");
    assert_eq!(doc["kind"], "modeldiff");
    assert_eq!(doc["status"], "ok");
    assert_eq!(doc["changed"][0]["name"], "claims");
    assert_eq!(doc["changed"][0]["classes"][0], "formula");
    assert_eq!(doc["changed"][0]["expr_nodes"][0]["change"], "replaced");
    assert_eq!(
        doc["changed"][0]["expr_nodes"][0]["before"],
        "survivors * q"
    );

    // `claims` feeds `net_cashflow` feeds `bel`, and both are Outputs.
    let impacted: Vec<&str> = doc["impact"]["impacted"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(impacted, vec!["bel", "net_cashflow"]);
    assert_eq!(doc["summary"]["outputs_affected"], 3);

    // The same run with the gate on is a domain failure.
    let (code, _, _) = cli(&[
        "diff",
        "model",
        &a.display().to_string(),
        &b.display().to_string(),
        "--fail-on-change",
    ]);
    assert_eq!(code, 1);
}

#[test]
fn a_renamed_component_is_reported_as_a_rename_not_as_add_plus_remove() {
    let (a, b) = pair(
        "rename",
        &[
            ("name = \"premium_income\"", "name = \"premiums\""),
            ("claims - premium_income", "claims - premiums"),
        ],
    );
    let (code, stdout, _) = cli(&[
        "diff",
        "model",
        &a.display().to_string(),
        &b.display().to_string(),
        "--json",
    ]);
    assert_eq!(code, 0);
    let doc = json_of(&stdout);
    assert_eq!(doc["renamed"][0]["from"], "premium_income");
    assert_eq!(doc["renamed"][0]["to"], "premiums");
    assert_eq!(doc["summary"]["added"], 0);
    assert_eq!(doc["summary"]["removed"], 0);
    assert_eq!(doc["summary"]["changed"], 0);
}

#[test]
fn a_doc_only_change_is_labelled_and_reaches_nothing() {
    let (a, b) = pair(
        "doc-only",
        &[(
            "expr = \"sum(net_cashflow)\"",
            "expr = \"sum(net_cashflow)\"\ndoc = \"Best estimate liability.\"",
        )],
    );
    let (code, stdout, _) = cli(&[
        "diff",
        "model",
        &a.display().to_string(),
        &b.display().to_string(),
        "--json",
    ]);
    assert_eq!(code, 0);
    let doc = json_of(&stdout);
    assert_eq!(doc["changed"][0]["classes"][0], "doc-only");
    assert_eq!(doc["summary"]["doc_only"], 1);
    assert_eq!(doc["summary"]["components_impacted"], 0);

    // A doc change is not a semantic change, so the CI gate stays green.
    let (code, _, _) = cli(&[
        "diff",
        "model",
        &a.display().to_string(),
        &b.display().to_string(),
        "--fail-on-change",
    ]);
    assert_eq!(code, 0);
}

#[test]
fn the_subject_word_is_required_and_arity_is_checked() {
    let (a, b) = pair("usage", &[]);
    // No subject: the run diff is still a stub, and says so.
    let (code, stdout, _) = cli(&[
        "diff",
        &a.display().to_string(),
        &b.display().to_string(),
        "--json",
    ]);
    assert_eq!(code, 2);
    assert_eq!(json_of(&stdout)["status"], "unimplemented");

    // One path is a usage error, not a comparison against nothing.
    let (code, _, stderr) = cli(&["diff", "model", &a.display().to_string()]);
    assert_eq!(code, 2);
    assert!(stderr.contains("two paths"), "{stderr}");

    // An unknown flag is a usage error, never a positional.
    let (code, _, stderr) = cli(&[
        "diff",
        "model",
        &a.display().to_string(),
        &b.display().to_string(),
        "--tolerance-profile",
        "reconcile",
    ]);
    assert_eq!(code, 2);
    assert!(stderr.contains("unknown flag"), "{stderr}");
}

#[test]
fn out_json_writes_the_same_document_to_a_file() {
    let (a, b) = pair(
        "out-json",
        &[(
            "expr = \"survivors * premium\"",
            "expr = \"survivors * premium * 1.1\"",
        )],
    );
    let out = a.parent().unwrap().join("diff.json");
    let (code, _, _) = cli(&[
        "diff",
        "model",
        &a.display().to_string(),
        &b.display().to_string(),
        "--out-json",
        &out.display().to_string(),
    ]);
    assert_eq!(code, 0);
    let doc = json_of(&fs::read_to_string(&out).unwrap());
    assert_eq!(doc["kind"], "modeldiff");
    assert_eq!(doc["changed"][0]["name"], "premium_income");
}
