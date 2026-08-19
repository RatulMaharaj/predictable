//! The hypothesis detectors of `04-verify.md` §5.5, against runs of the real reference model.
//!
//! The mutation harness (`mutation_harness.rs`) measures the detectors *in aggregate*, by mutating
//! model source and re-running the engine. These tests do the complementary job: they seed one
//! precisely-shaped divergence into a copy of the committed reference run and pin what the detector
//! says about it — the code, the evidence, the support counts, and the edit it proposes.
//!
//! Seeding at the result level rather than the model level is deliberate here: a constant `× -1`
//! on one component is a sign convention, and no legal `.pir` edit produces exactly that without
//! also producing something else.

mod support;

use predictable_rundiff::hypothesis::Confidence;
use predictable_rundiff::model::Class;
use predictable_rundiff::{diff_runs, DiffOptions, Finding, Hypothesis, Profile};
use support::*;

const EXPENSES: &str = "model.renewal_expenses";
const PREMIUMS: &str = "model.premium_income";
const DEATHS: &str = "model.deaths";
const BEL: &str = "model.bel";

/// Every difference is a real difference: `exact` so nothing is absorbed before a detector sees it.
fn exact() -> DiffOptions {
    DiffOptions {
        profile: Profile::builtin("exact").expect("exact is a builtin"),
        top: 0,
        ..options()
    }
}

fn find<'f>(findings: &'f [Finding], component: &str) -> &'f Finding {
    findings
        .iter()
        .find(|f| f.component == component)
        .unwrap_or_else(|| panic!("no finding for {component}"))
}

fn code<'f>(f: &'f Finding, code: &str) -> &'f Hypothesis {
    f.hypotheses
        .iter()
        .find(|h| h.code == code)
        .unwrap_or_else(|| {
            panic!(
                "{code} did not fire on {}; got {:?}",
                f.component,
                f.hypotheses.iter().map(|h| h.code).collect::<Vec<_>>()
            )
        })
}

// ---------------------------------------------------------------------------
// H0101 / H0103 — the ratio family
// ---------------------------------------------------------------------------

#[test]
fn h0101_reports_the_ratio_and_names_it_when_it_is_a_known_constant() {
    let a = base("a");
    let b = mutated("h0101", "b", |id, _, _, v| {
        Some(if id == EXPENSES { v / 12.0 } else { v })
    });
    let diff = diff_runs(&a, &b, &exact());
    let h = code(find(&diff.findings, EXPENSES), "H0101");

    assert_eq!(h.evidence["matches"]["name"], "1/12");
    assert!((h.evidence["ratio"].as_f64().unwrap() - 1.0 / 12.0).abs() < 1e-12);
    // Every diverging modelpoint carries the same claim, so support is total and confidence high.
    assert_eq!(h.confidence, Confidence::High);
    assert_eq!(h.evidence_support.held, h.evidence_support.tested);
    assert_eq!(
        h.evidence_support.tested, 24,
        "25 modelpoints, less the exemplar"
    );
    assert!(h.message.contains("1/12"));
}

#[test]
fn h0103_calls_a_sign_flip_a_sign_flip_and_h0101_stands_down() {
    let a = base("a");
    let b = mutated("h0103", "b", |id, _, _, v| {
        Some(if id == DEATHS { -v } else { v })
    });
    let diff = diff_runs(&a, &b, &exact());
    let f = find(&diff.findings, DEATHS);
    let h = code(f, "H0103");
    assert!(h.message.contains("sign = -1"));
    // A named cause explains the ratio better than "the ratio is constant".
    assert!(
        !f.hypotheses.iter().any(|h| h.code == "H0101"),
        "{:?}",
        f.hypotheses.iter().map(|h| h.code).collect::<Vec<_>>()
    );
}

#[test]
fn a_ratio_that_holds_on_only_half_the_population_is_reported_as_low_confidence() {
    // Half the population is scaled and half is shifted. §5.5 emits the hypothesis anyway — an
    // agent can falsify it in one run — but the support counts have to say plainly that it held on
    // half the sample, or "confidence" would be a word with no referent.
    let a = base("a");
    let b = mutated("h0101-split", "b", |id, mp, _, v| {
        Some(match (id, mp % 2) {
            (EXPENSES, 0) => v * 2.0,
            (EXPENSES, _) => v + 7.0,
            _ => v,
        })
    });
    let diff = diff_runs(&a, &b, &exact());
    let h = code(find(&diff.findings, EXPENSES), "H0101");
    assert_eq!(h.confidence, Confidence::Low);
    assert!(
        h.evidence_support.held * 2 <= h.evidence_support.tested,
        "{:?}",
        h.evidence_support
    );
}

// ---------------------------------------------------------------------------
// H0102 — constant offset, on a PerMP component
// ---------------------------------------------------------------------------

#[test]
fn h0102_works_across_the_population_for_a_per_mp_component() {
    // `bel` has one cell per modelpoint and no series at all: its divergence vector is the
    // population, and the detector has to read it that way or say nothing useful about the BEL.
    let a = base("a");
    let b = mutated("h0102", "b", |id, _, _, v| {
        Some(if id == BEL { v + 100.0 } else { v })
    });
    let diff = diff_runs(&a, &b, &exact());
    let h = code(find(&diff.findings, BEL), "H0102");
    assert_eq!(h.evidence["offset"].as_f64().unwrap(), 100.0);
    assert!(h.evidence_support.tested > 0);
    assert_eq!(h.evidence_support.held, h.evidence_support.tested);
}

// ---------------------------------------------------------------------------
// H0201 — off by one in `t`
// ---------------------------------------------------------------------------

#[test]
fn h0201_finds_a_one_period_shift_and_says_which_way() {
    let a = base("a");
    let source = base("source");
    let shifted = source.components.get(PREMIUMS).cloned().unwrap();
    let b = mutated("h0201", "b", move |id, mp, t, v| {
        if id != PREMIUMS {
            return Some(v);
        }
        // b[t] = a[t-1]: b lags a by one period.
        shifted
            .get(&(mp, t - 1))
            .and_then(|v| v.as_f64())
            .or(Some(v))
    });
    let diff = diff_runs(&a, &b, &exact());
    let h = code(find(&diff.findings, PREMIUMS), "H0201");
    assert_eq!(h.evidence["shift"], -1);
    assert!(h.message.contains("timing_shift"));
}

// ---------------------------------------------------------------------------
// H0402 — rounding
// ---------------------------------------------------------------------------

#[test]
fn h0402_names_the_precision_and_suggests_a_tolerance_rather_than_an_edit() {
    let a = base("a");
    let b = mutated("h0402", "b", |id, _, _, v| {
        Some(if id == EXPENSES {
            (v * 100.0).round() / 100.0
        } else {
            v
        })
    });
    let diff = diff_runs(&a, &b, &exact());
    let h = code(find(&diff.findings, EXPENSES), "H0402");
    assert_eq!(h.evidence["dp"], 2);
    assert_eq!(h.evidence["rounded_side"], "b");
    // §5.5: rounding is a tolerance question, so there is deliberately no model edit to apply.
    assert!(h.suggested_edit.is_none());
}

// ---------------------------------------------------------------------------
// H0501 — a trap, never a tolerance question
// ---------------------------------------------------------------------------

#[test]
fn h0501_fires_on_a_non_finite_value_whatever_the_profile_says() {
    let a = base("a");
    let b = mutated("h0501", "b", |id, _, t, v| {
        Some(if id == EXPENSES && t == 5 {
            f64::NAN
        } else {
            v
        })
    });
    // The loosest builtin profile: a NaN is still a finding, because NaN matches nothing (§5.6).
    let opts = DiffOptions {
        profile: Profile::builtin("materiality").expect("materiality is a builtin"),
        top: 0,
        ..options()
    };
    let diff = diff_runs(&a, &b, &opts);
    let h = code(find(&diff.findings, EXPENSES), "H0501");
    assert_eq!(h.evidence["side"], "b");
    assert_eq!(h.evidence["t"], 5);
    assert!(h.message.contains("trap"));
}

// ---------------------------------------------------------------------------
// H0303 — only one segment of the population diverges
// ---------------------------------------------------------------------------

#[test]
fn h0303_names_the_field_and_value_that_discriminates_the_diverging_modelpoints() {
    // A segment marker the run itself records: `surrenders` is flattened to a per-modelpoint
    // constant taking three values, and only the modelpoints carrying `0` have their expenses
    // moved. The detector has to find that field rather than merely say "some modelpoints".
    let a = base("a");
    let b = mutated("h0303", "b", |id, mp, _, v| {
        Some(match id {
            "model.surrenders" => f64::from(mp % 3),
            EXPENSES if mp % 3 == 0 => v * 1.5,
            _ => v,
        })
    });
    let diff = diff_runs(&a, &b, &exact());
    let h = code(find(&diff.findings, EXPENSES), "H0303");
    assert_eq!(h.evidence["field"], "model.surrenders");
    assert_eq!(h.evidence["value"], "0.0");
    let with = h.evidence["modelpoints_with_value"].as_u64().unwrap();
    let total = h.evidence["modelpoints_total"].as_u64().unwrap();
    assert!(with > 0 && with < total, "{with} of {total}");
}

// ---------------------------------------------------------------------------
// suggested edits
// ---------------------------------------------------------------------------

#[test]
fn a_suggested_edit_quotes_the_bytes_that_are_there_and_applies_to_the_file_on_disk() {
    let a = base("a");
    let b = mutated("edit", "b", |id, _, _, v| {
        Some(if id == EXPENSES { v * 2.0 } else { v })
    });
    let diff = diff_runs(&a, &b, &exact());
    let h = code(find(&diff.findings, EXPENSES), "H0101");
    let edit = h
        .suggested_edit
        .as_ref()
        .expect("a ratio on a component with a source anchor proposes a scale edit");

    let text = std::fs::read_to_string(&edit.file).expect("the anchored file exists");
    assert!(
        edit.still_applies(&text),
        "the edit's `old` is not the bytes at its range"
    );
    let after = edit.apply(&text).expect("the edit applies");
    // b is twice a, so the edit that makes b reproduce a divides by two — not multiplies.
    assert!(after.contains(") / 2"), "{after}");
    // The diagnostics-crate shape is the same edit, so one code path applies both.
    assert_eq!(edit.to_edit().range(), edit.byte_start..edit.byte_end);
}

#[test]
fn without_model_sources_the_hypothesis_is_still_made_but_carries_no_edit() {
    let a = base("a");
    let b = mutated("no-source", "b", |id, _, _, v| {
        Some(if id == EXPENSES { v * 2.0 } else { v })
    });
    // No model on either side: no graph, no attribution, no source anchors.
    let opts = DiffOptions {
        profile: Profile::builtin("exact").expect("exact is a builtin"),
        top: 0,
        ..DiffOptions::default()
    };
    let diff = diff_runs(&a, &b, &opts);
    let f = find(&diff.findings, EXPENSES);
    let h = code(f, "H0101");
    assert!(h.suggested_edit.is_none());
    // The ratio is a fact about the numbers, so it is still reported — just unnamed.
    assert!(h.evidence["matches"].is_null());
}

// ---------------------------------------------------------------------------
// the contract with the rest of the report
// ---------------------------------------------------------------------------

#[test]
fn hypotheses_are_deterministic_and_are_not_proposed_for_tolerance_only_findings() {
    let a = base("a");
    let b = mutated("determinism", "b", |id, _, _, v| {
        Some(if id == EXPENSES { v * 3.0 } else { v })
    });
    let first = diff_runs(&a, &b, &exact());
    let second = diff_runs(&a, &b, &exact());
    assert_eq!(
        serde_json::to_string(&first.to_json()).unwrap(),
        serde_json::to_string(&second.to_json()).unwrap()
    );
    for f in &first.findings {
        if f.class == Class::ToleranceOnly || f.class == Class::Structural {
            assert!(f.hypotheses.is_empty(), "{}", f.component);
        }
    }
}

#[test]
fn the_terminal_render_prints_the_code_the_support_and_the_edit() {
    let a = base("a");
    let b = mutated("render", "b", |id, _, _, v| {
        Some(if id == EXPENSES { v * 2.0 } else { v })
    });
    let diff = diff_runs(&a, &b, &exact());
    let text = predictable_rundiff::render::render(&diff);
    assert!(text.contains("H0101"), "{text}");
    assert!(text.contains("confidence, held on"), "{text}");
    assert!(text.contains("suggested edit"), "{text}");
}
