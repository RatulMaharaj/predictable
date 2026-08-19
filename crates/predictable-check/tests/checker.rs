//! Behavioural tests for the checker: what it reports, where, and what fix it
//! offers. The conformance corpus pins the contract; these pin the behaviour
//! around it — the cases a corpus of one-error-per-case cannot express, and the
//! properties (suggestions apply, nothing is double-reported) that hold for
//! every model rather than for one.

use predictable_check::{check, CheckResult, Input};
use predictable_diagnostics::{apply_edits, Diagnostic, Severity};

const HEADER: &str = r#"
format = "pir/1"
module = "t"

[timeline]
basis = "annual"
periods = 10
origin = "policy"
valuation_date = 2026-06-30
year_convention = "act/365"

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
name = "opt_loading"
dtype = "f64"
unit = "factor"
default = 1.0

[[assumption]]
name = "valuation_rate"
dtype = "f64"
unit = "rate(annual)"
shape = "Scalar"
"#;

fn model(components: &str) -> CheckResult {
    check(&[Input::new("model.pir", format!("{HEADER}{components}"))])
}

fn codes(result: &CheckResult) -> Vec<&str> {
    result.diagnostics.iter().map(|d| d.code.as_str()).collect()
}

fn only(result: &CheckResult, code: &str) -> Diagnostic {
    let found: Vec<&Diagnostic> = result
        .diagnostics
        .iter()
        .filter(|d| d.code == code)
        .collect();
    assert_eq!(
        found.len(),
        1,
        "expected exactly one {code}, got {:?}",
        codes(result)
    );
    found[0].clone()
}

/// Apply a diagnostic's first suggestion and re-check: the migration loop of
/// `01-ir.md` §7 in miniature.
fn apply_first_fix(source: &str, d: &Diagnostic) -> String {
    apply_edits(source, &d.suggestions[0].edits).expect("suggestion applies")
}

// ---------------------------------------------------------------------------
// pass 2 — resolve
// ---------------------------------------------------------------------------

#[test]
fn a_typo_gets_a_namespace_restricted_suggestion() {
    let result = model(
        r#"
[[component]]
name = "bel"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum_assure * 0.1"
"#,
    );
    let d = only(&result, "E0203");
    assert!(d.message.contains("sum_assure"));
    assert_eq!(d.suggestions[0].edits[0].replacement, "sum_assured");
}

#[test]
fn a_misspelled_table_is_never_corrected_to_a_component() {
    // `@` and `(` put tables and components in different namespaces (§4.2), so
    // the suggestion pool follows the use site.
    let result = check(&[Input::new(
        "model.pir",
        format!(
            "{HEADER}{}",
            r#"
[[table]]
name = "lapse_rates"
keys = [{ name = "policy_year", dtype = "i64", policy = "step" }]
values = [{ name = "lapse_pa", dtype = "f64", unit = "prob" }]
on_missing = "error"
source = "tables/lapses.csv"

[[component]]
name = "wx"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "prob"
timing = "end"
expr = "lapse_rate@(policy_year)"
"#
        ),
    )]);
    let d = only(&result, "E0203");
    assert_eq!(d.suggestions[0].edits[0].replacement, "lapse_rates");
}

#[test]
fn one_typo_is_one_diagnostic() {
    // The later passes treat an unresolved name as unknown rather than
    // re-reporting it, so a single typo does not cascade into a unit error and
    // a shape error as well.
    let result = model(
        r#"
[[component]]
name = "bel"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum_assure + valuation_rate"
"#,
    );
    assert_eq!(codes(&result), vec!["E0203"]);
}

// ---------------------------------------------------------------------------
// pass 3 — cycles
// ---------------------------------------------------------------------------

#[test]
fn a_lagged_self_reference_is_legal() {
    let result = model(
        r#"
[[component]]
name = "reserve"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "point"
init = "sum_assured"
expr = "reserve[t-1] * (1 + valuation_rate)"
"#,
    );
    assert!(result.is_ok(), "{:?}", codes(&result));
}

#[test]
fn a_same_period_cycle_suggests_a_lag() {
    let source = format!(
        "{HEADER}{}",
        r#"
[[component]]
name = "a"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "point"
expr = "b + 1.0"

[[component]]
name = "b"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "point"
expr = "a + 1.0"
"#
    );
    let result = check(&[Input::new("model.pir", source.clone())]);
    let d = only(&result, "E0201");
    assert!(
        d.spans.iter().filter(|s| !s.primary).count() >= 1,
        "a cycle names both ends"
    );
    let fixed = apply_first_fix(&source, &d);
    assert!(fixed.contains("b[t-1] + 1.0"));
    // And the fix actually resolves the cycle.
    assert!(check(&[Input::new("model.pir", fixed)]).is_ok());
}

#[test]
fn an_at_zero_self_reference_is_a_seed_but_a_later_one_is_a_cycle() {
    let seed = model(
        r#"
[[component]]
name = "x"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "point"
init = "sum_assured"
expr = "x[0] * 1.05"
"#,
    );
    assert!(seed.is_ok(), "{:?}", codes(&seed));

    let cycle = model(
        r#"
[[component]]
name = "x"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "point"
init = "sum_assured"
expr = "x[5] * 1.05"
"#,
    );
    let d = only(&cycle, "E0201");
    assert!(
        d.spans[0].label.as_deref().unwrap().contains("t = 5"),
        "the message names the period the edge is instantaneous at"
    );
}

// ---------------------------------------------------------------------------
// pass 4 — shapes
// ---------------------------------------------------------------------------

#[test]
fn narrowing_needs_an_aggregate_and_the_fix_type_checks() {
    let source = format!(
        "{HEADER}{}",
        r#"
[[component]]
name = "flow"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "sum_assured * 0.01"

[[component]]
name = "total"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "flow"
"#
    );
    let result = check(&[Input::new("model.pir", source.clone())]);
    let d = only(&result, "E0301");
    let fixed = apply_first_fix(&source, &d);
    assert!(fixed.contains("sum(flow)"));
    assert!(check(&[Input::new("model.pir", fixed)]).is_ok());
}

#[test]
fn widening_is_free() {
    let result = model(
        r#"
[[component]]
name = "series_from_permp"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "sum_assured * valuation_rate"
"#,
    );
    assert!(result.is_ok(), "{:?}", codes(&result));
}

// ---------------------------------------------------------------------------
// pass 5 — units, dtypes, presence
// ---------------------------------------------------------------------------

#[test]
fn literals_are_unit_polymorphic() {
    // `1 - prob` is `prob`, and `money + 100.0` is `money`: a literal unifies
    // with its context rather than clashing with it (§2.4).
    let result = model(
        r#"
[[component]]
name = "survival"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "prob"
expr = "1 - 0.01"

[[component]]
name = "loaded"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum_assured + 100.0"
"#,
    );
    assert!(result.is_ok(), "{:?}", codes(&result));
}

#[test]
fn money_times_money_is_reported_but_money_times_prob_is_not() {
    let result = model(
        r#"
[[component]]
name = "square"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum_assured * sum_assured"

[[component]]
name = "expected_claim"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum_assured * 0.01"
"#,
    );
    assert_eq!(codes(&result), vec!["E0503"]);
}

#[test]
fn is_null_is_legal_on_an_optional_field_and_not_on_a_required_one() {
    let result = model(
        r#"
[[component]]
name = "loading_absent"
kind = "Output"
dtype = "bool"
shape = "PerMP"
unit = "none"
expr = "is_null(opt_loading)"

[[component]]
name = "assured_absent"
kind = "Output"
dtype = "bool"
shape = "PerMP"
unit = "none"
expr = "is_null(sum_assured)"
"#,
    );
    let d = only(&result, "E0602");
    assert!(d.message.contains("sum_assured"));
    assert_eq!(d.suggestions[0].edits[0].replacement, "false");
}

#[test]
fn a_basis_conversion_written_as_division_offers_both_fixes() {
    let source = r#"
format = "pir/1"
module = "t"

[timeline]
basis = "monthly"
periods = 120
origin = "policy"
valuation_date = 2026-06-30

[[modelpoint_field]]
name = "policy_number"
dtype = "str"
required = true
key = true

[[assumption]]
name = "rate_pa"
dtype = "f64"
unit = "rate(annual)"
shape = "Scalar"

[[component]]
name = "monthly_rate"
kind = "Output"
dtype = "f64"
shape = "Scalar"
unit = "rate(monthly)"
expr = "rate_pa / 12"
"#;
    let result = check(&[Input::new("model.pir", source)]);
    let d = only(&result, "E1402");
    assert_eq!(
        d.suggestions.len(),
        2,
        "compounding first, then the named division"
    );
    assert_eq!(d.suggestions[0].edits[0].replacement, "to_monthly(rate_pa)");
    assert_eq!(
        d.suggestions[1].edits[0].replacement,
        "nominal_to_periodic(rate_pa, 12)"
    );
    assert!(check(&[Input::new("model.pir", apply_first_fix(source, &d))]).is_ok());
}

#[test]
fn an_explicit_conversion_is_not_reported() {
    let result = check(&[Input::new(
        "model.pir",
        r#"
format = "pir/1"
module = "t"

[timeline]
basis = "monthly"
periods = 120
origin = "policy"
valuation_date = 2026-06-30

[[modelpoint_field]]
name = "policy_number"
dtype = "str"
required = true
key = true

[[assumption]]
name = "rate_pa"
dtype = "f64"
unit = "rate(annual)"
shape = "Scalar"

[[component]]
name = "monthly_rate"
kind = "Output"
dtype = "f64"
shape = "Scalar"
unit = "rate(monthly)"
expr = "nominal_to_periodic(rate_pa, 12)"
"#,
    )]);
    assert!(result.is_ok(), "{:?}", codes(&result));
}

// ---------------------------------------------------------------------------
// pass 6 — lints
// ---------------------------------------------------------------------------

#[test]
fn lints_are_silent_while_the_model_has_errors() {
    // `never_used` would earn W0101, but the model does not check: telling the
    // author about an unused component underneath a real error is noise.
    let result = model(
        r#"
[[component]]
name = "never_used"
kind = "Derived"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum_assured * 2.0"

[[component]]
name = "broken"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "nonexistent"
"#,
    );
    assert_eq!(codes(&result), vec!["E0203"]);
}

#[test]
fn lints_are_warnings_and_do_not_block_a_run() {
    let result = model(
        r#"
[[component]]
name = "never_used"
kind = "Derived"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum_assured * 2.0"
"#,
    );
    assert_eq!(codes(&result), vec!["W0101"]);
    assert!(result.is_ok());
    assert_eq!(result.exit_code(), 1);
    assert_eq!(result.diagnostics[0].severity, Severity::Warning);
}

#[test]
fn a_lagged_sum_of_differently_timed_flows_is_not_a_timing_mismatch() {
    // W0103 is about *this* period's flows. `premium[t-1] + claims[t-1]` reads a
    // period that has closed, where the timing question does not arise.
    let result = model(
        r#"
[[component]]
name = "premium"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "sum_assured * 0.01"

[[component]]
name = "claims"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "sum_assured * 0.002"

[[component]]
name = "rolled"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "point"
expr = "premium[t-1] - claims[t-1]"
"#,
    );
    assert!(!codes(&result).contains(&"W0103"), "{:?}", codes(&result));
}

// ---------------------------------------------------------------------------
// pass 7 — run scope
// ---------------------------------------------------------------------------

#[test]
fn a_model_checks_without_a_run_file() {
    // §7: the seventh pass never runs at model-check time, so a model stays
    // checkable on its own.
    let result = model(
        r#"
[[component]]
name = "bel"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum_assured * 0.1"
"#,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", codes(&result));
}

#[test]
fn a_run_files_solve_and_aggregation_ids_must_resolve() {
    let model_text = format!(
        "{HEADER}{}",
        r#"
[[component]]
name = "bel"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum_assured * 0.1"
"#
    );
    let result = check(&[
        Input::new("model.pir", model_text),
        Input::new(
            "run.pir",
            r#"
format = "pir/1"

[run]
product = "product"
modelpoints = "data/mp.csv"
out = "runs/x"
emit = "outputs"

[[solve]]
name = "s"
target = "bel"
to = 0.0
vary = "no_such_field"
"#,
        ),
    ]);
    let d = only(&result, "E0203");
    assert!(d.message.contains("no_such_field"));
}
