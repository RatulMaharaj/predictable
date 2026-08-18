//! The diff against the real reference models.
//!
//! Every case here is a *variant* of `models/term_annual` built by editing one
//! thing in its committed canonical `.pir` — the same edits a reviewer makes in
//! anger: a loading nudged, a lag introduced, a component renamed, a timing
//! flipped, a mortality rate moved. The assertion in each case is not "the diff
//! is non-empty" but "the diff says exactly what was done and names exactly what
//! it reaches", which is the only claim worth making about a diff.

use std::fs;
use std::path::{Path, PathBuf};

use predictable_modeldiff::{diff, report, ChangeClass, ModelSide, NodeChangeKind};

fn reference_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../models/term_annual")
        .canonicalize()
        .expect("the term_annual reference model is committed")
}

/// Copy `models/term_annual/build/*.pir` plus its tables into a temp dir, then
/// apply `edit` to one file's text.
fn variant(dir: &Path, edits: &[(&str, &str, &str)]) -> PathBuf {
    let src = reference_dir();
    let out = dir.to_path_buf();
    fs::create_dir_all(out.join("tables")).unwrap();
    for name in ["model.pir", "schema.pir", "product.pir"] {
        let mut text = fs::read_to_string(src.join("build").join(name)).unwrap();
        for (file, from, to) in edits {
            if *file == name {
                assert!(text.contains(from), "{name} does not contain `{from}`");
                text = text.replace(from, to);
            }
        }
        fs::write(out.join(name), text).unwrap();
    }
    fs::write(
        out.join("base.pir"),
        fs::read(src.join("base.pir")).unwrap(),
    )
    .unwrap();
    for csv in ["mortality.csv", "lapses.csv", "expenses.csv"] {
        let mut text = fs::read_to_string(src.join("tables").join(csv)).unwrap();
        for (file, from, to) in edits {
            if *file == csv {
                assert!(text.contains(from), "{csv} does not contain `{from}`");
                text = text.replace(from, to);
            }
        }
        fs::write(out.join("tables").join(csv), text).unwrap();
    }
    out
}

fn pir_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "pir"))
        .collect();
    files.sort();
    files
}

fn diff_variants(
    a_edits: &[(&str, &str, &str)],
    b_edits: &[(&str, &str, &str)],
) -> predictable_modeldiff::ModelDiff {
    let tmp = tempfile::tempdir().unwrap();
    let a = variant(&tmp.path().join("a"), a_edits);
    let b = variant(&tmp.path().join("b"), b_edits);
    let sa = ModelSide::load("a", &pir_files(&a)).unwrap();
    let sb = ModelSide::load("b", &pir_files(&b)).unwrap();
    let d = diff(&sa, &sb);
    assert!(d.unparsed.is_empty(), "every reference file parses");
    d
}

#[test]
fn the_reference_model_does_not_differ_from_itself() {
    let d = diff_variants(&[], &[]);
    assert!(d.is_empty(), "{}", report::render_text(&d));
    assert_eq!(d.impact.impacted.len(), 0);
    // And the model really was loaded, so "empty" is not "nothing was read".
    let tmp = tempfile::tempdir().unwrap();
    let a = variant(&tmp.path().join("a"), &[]);
    let side = ModelSide::load("a", &pir_files(&a)).unwrap();
    assert!(side.components().count() > 15);
    assert!(side.assumption_values.contains_key("valuation_rate"));
    assert!(side.table_rows.contains_key("mortality"));
}

#[test]
fn a_loading_change_is_a_formula_change_with_a_named_impact_set() {
    // qx = mortality@(...) * mortality_loading  →  * 1.10
    let d = diff_variants(
        &[],
        &[(
            "model.pir",
            "expr = \"mortality@(age, sex, smoker) * mortality_loading\"",
            "expr = \"mortality@(age, sex, smoker) * 1.10\"",
        )],
    );
    assert_eq!(d.changed.len(), 1);
    let c = &d.changed[0];
    assert_eq!(c.name, "qx");
    assert_eq!(c.classes, vec![ChangeClass::Formula]);
    assert_eq!(c.expr_nodes.len(), 1);
    assert_eq!(c.expr_nodes[0].path.to_string(), "expr.rhs");
    assert_eq!(c.expr_nodes[0].before, "mortality_loading");
    assert_eq!(c.expr_nodes[0].after, "1.1");

    // Everything mortality touches, and nothing else.
    assert!(d.impact.impacted.contains(&"num_pols_if".to_string()));
    assert!(d.impact.impacted.contains(&"death_claims".to_string()));
    assert!(d.impact.outputs_affected.contains(&"bel".to_string()));
    assert!(d.impact.outputs_affected.contains(&"reserve".to_string()));
    assert!(
        !d.impact.impacted.contains(&"age".to_string()),
        "an upstream component is not impacted"
    );
}

#[test]
fn a_lag_introduced_is_an_offset_change_not_a_rewrite() {
    let d = diff_variants(
        &[],
        &[(
            "model.pir",
            "expr = \"entry_age + t\"",
            "expr = \"entry_age + t[t-1]\"",
        )],
    );
    assert_eq!(d.changed.len(), 1);
    assert_eq!(d.changed[0].name, "age");
    assert_eq!(d.changed[0].expr_nodes[0].change, NodeChangeKind::Offset);
    assert_eq!(d.changed[0].expr_nodes[0].after, "t[t-1]");
}

#[test]
fn a_timing_flip_is_classified_as_timing_and_nothing_else() {
    let d = diff_variants(
        &[],
        &[(
            "model.pir",
            "unit = \"prob\"\ntiming = \"end\"\nexpr = \"lapses@(policy_year) * lapse_loading\"",
            "unit = \"prob\"\ntiming = \"start\"\nexpr = \"lapses@(policy_year) * lapse_loading\"",
        )],
    );
    assert_eq!(d.changed.len(), 1);
    assert_eq!(d.changed[0].name, "wx");
    assert_eq!(d.changed[0].classes, vec![ChangeClass::Timing]);
    assert!(d.changed[0].expr_nodes.is_empty());
    assert_eq!(d.changed[0].fields[0].before, "end");
}

#[test]
fn a_rename_is_reported_once_and_not_as_a_change_in_every_dependent() {
    // `qx` → `mortality_rate`, consistently, across every reference to it.
    let tmp = tempfile::tempdir().unwrap();
    let a = variant(&tmp.path().join("a"), &[]);
    let b = variant(&tmp.path().join("b"), &[]);
    // Rewrite whole-word `qx` references in the model, but not the table's
    // value column of the same name.
    let path = b.join("model.pir");
    let text = fs::read_to_string(&path).unwrap();
    let renamed = text
        .replace("name = \"qx\"", "name = \"mortality_rate\"")
        .replace("* qx", "* mortality_rate")
        .replace("(1 - qx)", "(1 - mortality_rate)")
        .replace("qx[t-1]", "mortality_rate[t-1]");
    assert_ne!(renamed, text);
    fs::write(&path, renamed).unwrap();

    let sa = ModelSide::load("a", &pir_files(&a)).unwrap();
    let sb = ModelSide::load("b", &pir_files(&b)).unwrap();
    let d = diff(&sa, &sb);

    assert_eq!(d.renamed.len(), 1, "{}", report::render_text(&d));
    assert_eq!(d.renamed[0].from, "qx");
    assert_eq!(d.renamed[0].to, "mortality_rate");
    assert_eq!(d.renamed[0].evidence, "expr+unit");
    assert!(d.added.is_empty() && d.removed.is_empty());
    assert!(
        d.changed.is_empty(),
        "a consistent rename changes nothing: {:?}",
        d.changed
    );
    assert!(d.impact.impacted.is_empty());
    assert!(d.is_semantically_empty());
}

#[test]
fn a_removed_output_is_reported_with_what_used_to_depend_on_it() {
    let d = diff_variants(&[], &[("model.pir", "kind = \"Output\"\ndtype = \"f64\"\nshape = \"PerMP\"\nunit = \"money\"\nexpr = \"pv_claims + pv_expenses + initial_expense - pv_premiums\"", "kind = \"Derived\"\ndtype = \"f64\"\nshape = \"PerMP\"\nunit = \"money\"\nexpr = \"pv_claims + pv_expenses + initial_expense - pv_premiums\"")]);
    assert_eq!(d.changed.len(), 1);
    assert_eq!(d.changed[0].name, "bel");
    assert_eq!(d.changed[0].classes, vec![ChangeClass::Kind]);
    // `bel` seeds `reserve` through its `init`.
    assert!(d.impact.impacted.contains(&"reserve".to_string()));
}

#[test]
fn a_changed_mortality_rate_is_a_row_diff_and_reaches_every_reader() {
    let d = diff_variants(
        &[],
        &[(
            "mortality.csv",
            "18,F,false,0.00017432",
            "18,F,false,0.00019",
        )],
    );
    assert_eq!(d.tables.len(), 1);
    let t = &d.tables[0];
    assert_eq!(t.name, "mortality");
    let rows = t.rows.as_ref().expect("both CSVs were readable");
    assert_eq!(rows.changed.len(), 1);
    assert_eq!(rows.changed[0].key, vec!["18", "F", "false"]);
    assert_eq!(rows.changed[0].before, vec!["0.00017432"]);
    assert_eq!(rows.changed[0].after, vec!["0.00019"]);
    assert!(rows.added.is_empty() && rows.removed.is_empty());
    // No component declaration moved, but the table's readers did.
    assert!(d.changed.is_empty());
    assert!(d.impact.impacted.contains(&"qx".to_string()));
    assert!(d.impact.outputs_affected.contains(&"bel".to_string()));
}

#[test]
fn an_assumption_value_change_is_reported_with_its_impact() {
    let d = diff_variants(&[], &[]);
    assert!(d.assumptions.is_empty());

    let tmp = tempfile::tempdir().unwrap();
    let a = variant(&tmp.path().join("a"), &[]);
    let b = variant(&tmp.path().join("b"), &[]);
    let path = b.join("base.pir");
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace("valuation_rate = 0.035", "valuation_rate = 0.04");
    fs::write(&path, text).unwrap();

    let sa = ModelSide::load("a", &pir_files(&a)).unwrap();
    let sb = ModelSide::load("b", &pir_files(&b)).unwrap();
    let d = diff(&sa, &sb);

    assert_eq!(d.assumptions.len(), 1);
    assert_eq!(d.assumptions[0].name, "valuation_rate");
    assert_eq!(d.assumptions[0].value_before.as_deref(), Some("0.035"));
    assert_eq!(d.assumptions[0].value_after.as_deref(), Some("0.04"));
    assert!(d.impact.impacted.contains(&"reserve".to_string()));
    assert!(d.impact.outputs_affected.contains(&"reserve".to_string()));
}

#[test]
fn the_json_document_round_trips() {
    let d = diff_variants(
        &[],
        &[(
            "model.pir",
            "expr = \"mortality@(age, sex, smoker) * mortality_loading\"",
            "expr = \"mortality@(age, sex, smoker) / mortality_loading\"",
        )],
    );
    let json = d.to_json();
    assert_eq!(json["changed"][0]["name"], "qx");
    assert_eq!(json["changed"][0]["classes"][0], "formula");
    assert_eq!(json["changed"][0]["expr_nodes"][0]["change"], "operator");
    assert_eq!(json["changed"][0]["expr_nodes"][0]["path"], "expr");

    let back: predictable_modeldiff::ModelDiff = serde_json::from_value(json).unwrap();
    assert_eq!(back, d);
}
