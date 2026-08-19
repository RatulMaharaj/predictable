//! Diffing a Prophet-imported run against a predictable run (`04-verify.md` §4.3.7, §5).
//!
//! The point of these tests is that there is no Prophet-specific comparison path: the `.rpt` is
//! imported into a real run directory and then diffed by the same code that diffs two predictable
//! runs. What *is* Prophet-specific is declared rather than inferred — the component
//! correspondence comes from `migration/mapping.toml`, and the file's own fixed precision raises
//! the tolerance for the columns it wrote.

mod support;

use std::collections::BTreeMap;

use predictable_prophet::rpt::{read_rpt, RptOptions};
use predictable_prophet::run_dir::{write_run_dir, ImportOptions};
use predictable_rundiff::model::{Class, Verdict};
use predictable_rundiff::{diff_runs, Mapping, RunSide, Value};
use support::*;

const EXPENSES: &str = "model.renewal_expenses";
const PREMIUM: &str = "model.premium_income";

const MAPPING: &str = r#"
format = "pvf/1"
prophet_run = "TERM_BASE_2026Q2"
model_module = "model"
period_base = 1
mp_key = { prophet = "POL_NUM", predictable = "policy_number" }

[[component]]
prophet = "PREM_INC"
predictable = "premium_income"
scale = 1000.0

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

/// A `.rpt` carrying the reference run's own numbers, written the way Prophet writes them:
/// 1-based periods, 2 decimal places, premiums in thousands, and one extra column that has no
/// predictable counterpart.
fn rpt_text(source: &RunSide, expense_factor: f64) -> String {
    let mut text = String::from(
        "! Prophet results\n\
         RUN, TERM_BASE_2026Q2\n\
         PRODUCT, TERM_UK\n\
         RUN_DATE, 30/06/2026\n\
         TIME_UNITS, YEARS\n\
         NUM_PERIODS, 40\n\
         POL_NUM, PERIOD, PREM_INC, DTH_CLAIM, EXPENSE, RESERVE_INT\n",
    );
    let at = |id: &str, mp: u32, t: i32| -> f64 {
        source
            .components
            .get(id)
            .and_then(|c| c.get(&(mp, t)))
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
    };
    for (mp_row, mp_key) in &source.mp_keys {
        for t in 0..=40 {
            let premium = at(PREMIUM, *mp_row, t) / 1000.0;
            let claims = at("model.death_claims", *mp_row, t);
            let expense = at(EXPENSES, *mp_row, t) * expense_factor;
            text.push_str(&format!(
                "{mp_key}, {}, {premium:.2}, {claims:.2}, {expense:.2}, 0.00\n",
                t + 1
            ));
        }
    }
    text
}

/// Import the `.rpt` under its *own* names (`prophet.PREM_INC`, …). The rename is the mapping
/// file's job at diff time, which is what these tests are here to exercise; an importer that had
/// already renamed the columns would hide whether the diff can do it at all.
fn prophet_run(tag: &str, source: &RunSide, expense_factor: f64) -> RunSide {
    let text = rpt_text(source, expense_factor);
    let read = read_rpt(
        text.as_bytes(),
        "term.rpt",
        &RptOptions {
            period_base: Some(1),
            timeline_basis: None,
            mp_key_column: Some("POL_NUM".to_string()),
            component_names: BTreeMap::new(),
        },
    );
    assert!(!read.has_errors(), "{:?}", read.diagnostics);
    let dir = scratch(tag).join("run_prophet");
    write_run_dir(
        &read.value,
        &dir,
        text.as_bytes(),
        &ImportOptions::default(),
    )
    .expect("the .rpt becomes a run directory");
    RunSide::load("a", &dir).expect("the imported run loads")
}

fn mapping(tag: &str) -> Mapping {
    let path = scratch(tag).join("mapping.toml");
    std::fs::write(&path, MAPPING).unwrap();
    Mapping::load(&path).expect("the mapping parses")
}

#[test]
fn a_faithful_prophet_export_reconciles_to_the_run_it_came_from() {
    let b = base("b");
    let map = mapping("prophet-clean-map");
    let a = prophet_run("prophet-clean", &b, 1.0);

    let mut opts = options_b_only();
    opts.mapping = Some(map);
    let diff = diff_runs(&a, &b, &opts);

    // Same numbers in different clothes: 1-based periods, thousands, 2dp. Every one of those is a
    // declared adjustment or a stated source precision, so the verdict is `matched`.
    assert_eq!(diff.verdict, Verdict::Matched, "{:?}", diff.headline());
    assert_eq!(diff.a_system, "prophet");
    assert_eq!(diff.b_system, "predictable");
    assert_eq!(diff.components.common, 3);
    assert_eq!(diff.modelpoints.common, 25);
    assert!(diff.cells.compared > 0);

    // Premiums came over in thousands at 2dp, so the file's own precision is worth half a
    // thousandth of a currency unit: the diff raises the tolerance for that column rather than
    // reporting a phantom divergence in every cell, and files the absorbed cells as their own
    // `tolerance-only` finding so the raise cannot pass unnoticed.
    let absorbed = diff
        .findings
        .iter()
        .find(|f| f.class == Class::ToleranceOnly)
        .expect("the raised tolerance is reported as a finding");
    assert_eq!(absorbed.component, PREMIUM);
    assert!(diff.cells.absorbed_by_override > 0);

    // The unmapped Prophet column is reported, with the reason the mapping file declared.
    let only_a: Vec<&str> = diff
        .structural
        .only_in_a
        .iter()
        .map(|o| o.component.as_str())
        .collect();
    assert_eq!(only_a, vec!["prophet.reserve_int"]);
    assert_eq!(diff.structural.only_in_a[0].reason, "declared unmapped");

    // The `× 1000` is printed in the header, because a mapping adjustment is a claim, not a fudge.
    let text = predictable_rundiff::render::render(&diff);
    assert!(text.contains("mapping adjustment"), "{text}");
    assert!(text.contains("PREM_INC"));
    assert!(text.contains("× 1000"));

    // 2dp in the source raised the tolerance for the columns it wrote, and said so.
    let raised = diff
        .tolerances
        .overrides_applied
        .iter()
        .filter(|o| o.note.as_deref() == Some("tolerance_raised_by_source_precision"))
        .count();
    assert!(raised > 0, "{:?}", diff.tolerances.overrides_applied);
}

#[test]
fn an_inflated_prophet_expense_column_localises_to_the_expense_component() {
    let b = base("b");
    let map = mapping("prophet-diverge-map");
    // The Prophet run applies one extra year of inflation to its expense column.
    let a = prophet_run("prophet-diverge", &b, 1.028);

    let mut opts = options_b_only();
    opts.mapping = Some(map);
    let diff = diff_runs(&a, &b, &opts);

    assert_eq!(diff.verdict, Verdict::Diverged);
    assert_eq!(diff.verdict.exit_code(), 1);
    // Two findings: the expense divergence, and the `tolerance-only` note for the premium column
    // whose thousands-at-2dp precision the diff raised the tolerance for.
    assert_eq!(
        diff.findings.len(),
        2,
        "{:?}",
        diff.findings
            .iter()
            .map(|f| (&f.component, f.class))
            .collect::<Vec<_>>()
    );
    let f = &diff.findings[0];
    assert_eq!(f.component, EXPENSES);
    assert_eq!(f.source_component.as_deref(), Some("prophet.expense"));
    assert_eq!(f.class, Class::Root);
    // t = 0 on the predictable side, which is PERIOD 1 in the file: the rebase happened at import,
    // so the diff never has to think about it.
    assert_eq!(f.t_first, 0);
    assert!(f
        .message
        .contains("diverges at t=0 in component renewal_expenses"));

    // Model attribution is unavailable — there is no Prophet model to diff — and the diff says so
    // rather than reporting the divergence as unexplained.
    assert!(!diff.model_attribution_available);
    assert!(f.explained_by_model_change.is_none());
}

#[test]
fn without_a_mapping_nothing_lines_up_and_the_diff_says_exactly_that() {
    let b = base("b");
    let a = prophet_run("prophet-nomap", &b, 1.0);

    // Same two runs, no mapping: component correspondence is data, and without the data there is
    // no correspondence to guess at.
    let diff = diff_runs(&a, &b, &options_b_only());
    assert_eq!(diff.components.common, 0);
    assert_eq!(diff.structural.only_in_a.len(), 4);
    assert_eq!(diff.structural.only_in_b.len(), 13);
    assert!(diff
        .structural
        .only_in_a
        .iter()
        .all(|o| o.reason == "no counterpart"));
    // Nothing compared, so nothing diverged: the report is about the sets, not the numbers.
    assert_eq!(diff.cells.compared, 0);
    assert_eq!(diff.verdict, Verdict::Matched);
}

#[test]
fn a_declared_timing_shift_moves_the_axis_rather_than_the_verdict() {
    let b = base("b");
    let map = mapping("prophet-shift-map");
    let text = rpt_text(&b, 1.0);
    // Re-import the same file with a period base of 0, so every `t` is one too large; a
    // `timing_shift = -1` in the mapping is the declared correction for exactly that.
    let read = read_rpt(
        text.as_bytes(),
        "term.rpt",
        &RptOptions {
            period_base: Some(0),
            timeline_basis: None,
            mp_key_column: Some("POL_NUM".to_string()),
            component_names: BTreeMap::new(),
        },
    );
    let dir = scratch("prophet-shift").join("run_prophet");
    write_run_dir(
        &read.value,
        &dir,
        text.as_bytes(),
        &ImportOptions::default(),
    )
    .unwrap();
    let a = RunSide::load("a", &dir).unwrap();

    let mut opts = options_b_only();
    opts.mapping = Some(map.clone());
    let unshifted = diff_runs(&a, &b, &opts);
    assert_eq!(unshifted.verdict, Verdict::Diverged);

    let mut shifted_map = map;
    for entry in &mut shifted_map.components {
        entry.timing_shift = -1;
    }
    let mut opts = options_b_only();
    opts.mapping = Some(shifted_map);
    let shifted = diff_runs(&a, &b, &opts);
    // The last period drops out on one side (t = 40 has no counterpart at t = 39 + 1), so what is
    // compared matches and what is not is reported as a missing cell rather than a value error.
    assert!(
        shifted.cells.diverged < unshifted.cells.diverged,
        "{} vs {}",
        shifted.cells.diverged,
        unshifted.cells.diverged
    );
    let text = predictable_rundiff::render::render(&shifted);
    assert!(text.contains("t -1"), "{text}");
}
