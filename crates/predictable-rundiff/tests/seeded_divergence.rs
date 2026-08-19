//! Seeded known-divergence pairs on the `term_annual` reference model, with exact expected
//! findings (`04-verify.md` §5).
//!
//! Each test injects one known defect into a copy of the committed reference run and then asserts
//! the *whole* localisation: which component is the root, at which `t`, on which modelpoint, what
//! is inherited from it, and how the report ranks them. A run diff that says "these two runs
//! differ" is worth nothing; these tests pin what it says instead.

mod support;

use predictable_rundiff::model::{Category, Class, Verdict};
use predictable_rundiff::tolerance::{Profile, Tol};
use predictable_rundiff::{diff_runs, DiffOptions, Finding};
use support::*;

const EXPENSES: &str = "model.renewal_expenses";
const NET: &str = "model.net_cashflow";
const BEL: &str = "model.bel";
const PV_EXPENSES: &str = "model.pv_expenses";
const RESERVE: &str = "model.reserve";

fn find<'f>(findings: &'f [Finding], component: &str) -> &'f Finding {
    findings
        .iter()
        .find(|f| f.component == component)
        .unwrap_or_else(|| {
            panic!(
                "no finding for {component}; got {:?}",
                findings.iter().map(|f| &f.component).collect::<Vec<_>>()
            )
        })
}

// ---------------------------------------------------------------------------
// the baseline: a run against itself
// ---------------------------------------------------------------------------

#[test]
fn a_run_against_itself_matches_and_says_nothing_else() {
    let a = base("a");
    let b = base("b");
    let diff = diff_runs(&a, &b, &options());
    assert_eq!(diff.verdict, Verdict::Matched);
    assert_eq!(diff.verdict.exit_code(), 0);
    assert!(diff.findings.is_empty(), "{:?}", diff.headline());
    assert_eq!(diff.cells.diverged, 0);
    assert_eq!(diff.cells.compared, a.rows as u64);
    assert_eq!(diff.components.only_a, 0);
    assert_eq!(diff.components.only_b, 0);
    assert_eq!(diff.modelpoints.common, 25);
    assert!(diff.first_divergence.is_none());
    assert!(diff.graph_available && diff.model_attribution_available);
}

// ---------------------------------------------------------------------------
// the headline case: one inflated component, and everything downstream of it
// ---------------------------------------------------------------------------

/// `renewal_expenses` inflated by one extra year of `expense_inflation` (2.8%) from `t = 3`, and
/// every component downstream of it moved with it.
///
/// The mutation is applied to the *stored results*, so the downstream cells have to be seeded
/// too — a stored-cell edit is not a re-projection. What the fixture buys is exactness: the
/// diff has one right answer about which of these five is the root, and the test names it. The
/// engine-backed version of the same defect (change the assumption, re-run, diff) lives in
/// `predictable-cli`'s `run_diff` tests.
fn inflated() -> (predictable_rundiff::RunSide, predictable_rundiff::RunSide) {
    let a = base("a");
    let b = mutated("inflated", "b", |id, _mp, t, v| {
        Some(match id {
            EXPENSES if t >= 3 => v * 1.028,
            NET if t >= 3 => v - 0.4,
            PV_EXPENSES | BEL => v + 3.0,
            RESERVE if t >= 4 => v + 1.5,
            _ => v,
        })
    });
    (a, b)
}

/// Only the root component moves — nothing downstream of it does.
fn root_only() -> (predictable_rundiff::RunSide, predictable_rundiff::RunSide) {
    let a = base("a");
    let b = mutated("root-only", "b", |id, _mp, t, v| {
        Some(if id == EXPENSES && t >= 3 {
            v * 1.028
        } else {
            v
        })
    });
    (a, b)
}

#[test]
fn the_root_is_the_component_that_changed_not_the_output_that_moved() {
    let (a, b) = inflated();
    let diff = diff_runs(&a, &b, &options());
    assert_eq!(diff.verdict, Verdict::Diverged);
    assert_eq!(diff.verdict.exit_code(), 1);

    let root = find(&diff.findings, EXPENSES);
    assert_eq!(root.class, Class::Root);
    // `partial_graph`, not `ir_graph`: this run emitted outputs only, so `expense_scale` and
    // `num_pols_if` — inputs of `renewal_expenses` — were never written and cannot be checked.
    // Saying which is what makes the classification falsifiable.
    assert_eq!(root.class_basis, "partial_graph");
    assert_eq!(root.category, Category::Value);
    assert_eq!(root.t_first, 3);
    assert_eq!(root.id, "F001", "the root sorts first");

    // Everything that reads it, directly or transitively, is inherited — never a second root.
    for downstream in [NET, PV_EXPENSES, BEL, RESERVE] {
        let f = find(&diff.findings, downstream);
        assert_eq!(
            f.class,
            Class::Inherited,
            "{downstream} should be inherited"
        );
        assert_eq!(f.class_basis, "ir_graph");
    }
    assert_eq!(diff.root_divergences, 1);
    assert_eq!(diff.findings.len(), 5);

    // Components the expense flow does not reach are untouched.
    assert!(diff
        .findings
        .iter()
        .all(|f| f.component != "model.premium_income"));
    assert!(diff.findings.iter().all(|f| f.component != "model.deaths"));
}

#[test]
fn the_earliest_divergence_is_the_earliest_t_not_the_largest_number() {
    let (a, b) = inflated();
    let diff = diff_runs(&a, &b, &options());
    let first = diff.first_divergence.as_ref().expect("a first divergence");
    // `bel` and `pv_expenses` are PerMP (t = -1) and would sort first on a naive numeric minimum,
    // so this pins that a `PerMP` aggregate never masquerades as the earliest *timestep*.
    assert_eq!(first.component, EXPENSES);
    assert_eq!(first.t, 3);

    let root = find(&diff.findings, EXPENSES);
    assert_eq!(root.t_first, 3);
    assert_eq!(root.t_range[0], 3);
    // The exemplar is the lowest `mp_row` exhibiting the divergence at `t_first` (§5.4).
    let at_first: Vec<u32> = root
        .cells
        .iter()
        .filter(|c| c.t == 3)
        .map(|c| c.mp_row)
        .collect();
    assert_eq!(root.exemplar.mp_row, *at_first.iter().min().unwrap());
    assert_eq!(root.exemplar.t, 3);
}

#[test]
fn the_message_is_the_one_line_signal_of_5_2() {
    let (a, b) = inflated();
    let diff = diff_runs(&a, &b, &options());
    let root = find(&diff.findings, EXPENSES);
    // "<output> diverges at t=<t> in component <component>: <a> vs <b>  (Δ, %)  [mp <key>]"
    assert!(
        root.message
            .contains("diverges at t=3 in component renewal_expenses:"),
        "{}",
        root.message
    );
    assert!(root
        .message
        .contains(&format!("[mp {}]", root.exemplar.mp_key)));
    assert_eq!(diff.headline(), Some(root.message.as_str()));
}

#[test]
fn only_the_component_that_moved_is_reported() {
    let (a, b) = root_only();
    let diff = diff_runs(&a, &b, &options());
    assert_eq!(diff.findings.len(), 1);
    assert_eq!(diff.findings[0].component, EXPENSES);
    assert_eq!(diff.findings[0].class, Class::Root);
    assert_eq!(diff.root_divergences, 1);
}

#[test]
fn the_root_carries_the_whole_movement_of_the_output_it_feeds() {
    let (a, b) = root_only();
    let diff = diff_runs(&a, &b, &options());
    let root = find(&diff.findings, EXPENSES);
    let contribution = root.contribution.as_ref().expect("a contribution");
    // Every penny of the headline output's movement comes from this one component, because it is
    // the only thing that changed.
    assert!(
        (contribution.share_of_total_delta - 1.0).abs() < 1e-9,
        "{contribution:?}"
    );
    assert_eq!(contribution.output, EXPENSES);
    assert!(root.affects_outputs.contains(&BEL.to_string()));
    assert!(root.affects_outputs.contains(&NET.to_string()));

    // The output total moves in the direction the mutation pushed it, and the outputs that did
    // not move are still reported as within tolerance rather than omitted.
    let expenses = diff
        .outputs
        .iter()
        .find(|o| o.component == EXPENSES)
        .expect("renewal_expenses is an output");
    assert!(!expenses.within_tolerance);
    assert!(expenses.b_total > expenses.a_total);
    let bel = diff.outputs.iter().find(|o| o.component == BEL).unwrap();
    assert!(bel.within_tolerance);
}

#[test]
fn a_model_change_that_explains_the_divergence_is_reported_as_explained() {
    let (a, b) = inflated();
    let files = model_files();
    // Side B's model is the reference model with the expense formula edited: the model diff should
    // then *explain* the root divergence rather than leaving it a mystery.
    let dir = scratch("explained");
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap();
        let text = text.replace(
            "renewal_expense_pa * compound(expense_inflation, t)",
            "renewal_expense_pa * compound(expense_inflation, t + 1)",
        );
        std::fs::write(dir.join(f.file_name().unwrap()), text).unwrap();
    }
    let edited: Vec<_> = files
        .iter()
        .map(|f| dir.join(f.file_name().unwrap()))
        .collect();
    let opts = DiffOptions::default().with_model_paths(Some(&files), Some(&edited));
    let diff = diff_runs(&a, &b, &opts);
    let root = find(&diff.findings, EXPENSES);
    let explained = root
        .explained_by_model_change
        .as_ref()
        .expect("both models were available");
    assert!(explained.changed);
    assert!(
        explained.what.contains(&"formula".to_string()),
        "{explained:?}"
    );

    // With identical models on both sides, the same divergence is *unexplained* — and unexplained
    // root divergence is promoted to the top of the report (§5.3 step 5).
    let same = diff_runs(&a, &b, &options());
    let root = find(&same.findings, EXPENSES);
    assert!(!root.explained_by_model_change.as_ref().unwrap().changed);
    assert_eq!(root.id, "F001");
}

#[test]
fn the_report_is_byte_identical_on_a_re_run() {
    let (a, b) = inflated();
    let one = diff_runs(&a, &b, &options());
    let two = diff_runs(&a, &b, &options());
    assert_eq!(
        serde_json::to_string(&one.to_json()).unwrap(),
        serde_json::to_string(&two.to_json()).unwrap()
    );
    assert_eq!(
        predictable_rundiff::render::render(&one),
        predictable_rundiff::render::render(&two)
    );
}

#[test]
fn diff_a_b_and_diff_b_a_report_the_same_divergences() {
    let (a, b) = inflated();
    let forward = diff_runs(&a, &b, &options());
    let backward = diff_runs(&b, &a, &options());
    assert_eq!(forward.verdict, backward.verdict);
    assert_eq!(forward.cells.diverged, backward.cells.diverged);
    assert_eq!(forward.cells.compared, backward.cells.compared);
    let names = |d: &predictable_rundiff::RunDiff| -> Vec<String> {
        d.findings.iter().map(|f| f.component.clone()).collect()
    };
    assert_eq!(names(&forward), names(&backward));
    assert_eq!(
        find(&forward.findings, EXPENSES).t_first,
        find(&backward.findings, EXPENSES).t_first
    );
}

// ---------------------------------------------------------------------------
// tolerance
// ---------------------------------------------------------------------------

#[test]
fn a_difference_below_the_profile_is_not_a_finding_and_above_it_is() {
    let a = base("a");
    // Half a cent on one component: inside `reconcile`, outside `exact`.
    let b = mutated("half-cent", "b", |id, _, t, v| {
        Some(if id == EXPENSES && t == 5 {
            v + 0.004
        } else {
            v
        })
    });

    let diff = diff_runs(&a, &b, &options());
    assert_eq!(diff.verdict, Verdict::Matched, "{:?}", diff.headline());

    let mut opts = options();
    opts.profile = Profile::builtin("exact").unwrap();
    let strict = diff_runs(&a, &b, &opts);
    assert_eq!(strict.verdict, Verdict::Diverged);
    let f = find(&strict.findings, EXPENSES);
    assert_eq!(f.t_first, 5);
    assert_eq!(f.n_cells, 25, "one cell per modelpoint at t = 5");
}

#[test]
fn a_loosened_per_component_tolerance_is_never_invisible() {
    let a = base("a");
    let b = mutated("loosened", "b", |id, _, t, v| {
        Some(if id == EXPENSES && t == 5 { v + 0.5 } else { v })
    });

    // Under the profile alone this diverges.
    assert_eq!(diff_runs(&a, &b, &options()).verdict, Verdict::Diverged);

    // Loosened for that one component, it does not — but the loosening is reported, in the JSON
    // and in the header, and the absorbed cells are counted.
    let mut opts = options();
    opts.profile = Profile::builtin("reconcile")
        .unwrap()
        .with_component(EXPENSES, Tol::new(1.0, 1e-6));
    let diff = diff_runs(&a, &b, &opts);
    assert_eq!(diff.verdict, Verdict::Matched);
    assert_eq!(diff.cells.absorbed_by_override, 25);

    let loosenings = diff.tolerances.loosenings();
    assert_eq!(loosenings.len(), 1);
    assert_eq!(loosenings[0].component, EXPENSES);
    assert!(loosenings[0].looser);
    assert!(predictable_rundiff::render::render(&diff).contains("tolerance loosened"));

    // And it is a finding in its own right, classed so it can never be mistaken for a divergence.
    let f = find(&diff.findings, EXPENSES);
    assert_eq!(f.class, Class::ToleranceOnly);
    assert_eq!(f.n_cells, 25);

    let json = diff.to_json();
    assert_eq!(json["tolerance"]["loosened"], 1);
    assert_eq!(json["summary"]["verdict"], "matched");
}

#[test]
fn a_probability_is_not_compared_to_the_half_cent() {
    // `reconcile`'s per-unit table is what stops a money tolerance being applied to `qx`.
    let profile = Profile::builtin("reconcile").unwrap();
    let money = profile.resolve("m.x", Some(&predictable_ir::Unit::Money), None);
    let prob = profile.resolve("m.qx", Some(&predictable_ir::Unit::Prob), None);
    assert_eq!(money.tol.abs, 0.005);
    assert_eq!(prob.tol.abs, 1e-9);
}

// ---------------------------------------------------------------------------
// NaN, missing cells, structural differences
// ---------------------------------------------------------------------------

#[test]
fn a_nan_is_a_finding_whatever_the_tolerance_says() {
    let a = base("a");
    let b = mutated("nan", "b", |id, _, t, v| {
        Some(if id == EXPENSES && t == 7 {
            f64::NAN
        } else {
            v
        })
    });
    let mut opts = options();
    // The loosest tolerance imaginable: a NaN is still not a match.
    opts.profile = Profile::custom(1e18, 1.0);
    let diff = diff_runs(&a, &b, &opts);
    assert_eq!(diff.verdict, Verdict::Diverged);
    let f = find(&diff.findings, EXPENSES);
    assert_eq!(f.category, Category::Nan);
    assert_eq!(f.t_first, 7);
}

#[test]
fn a_cell_present_on_one_side_only_is_missing_not_a_value_difference() {
    let a = base("a");
    let b = mutated("dropped", "b", |id, _, t, v| {
        if id == EXPENSES && t == 9 {
            None
        } else {
            Some(v)
        }
    });
    let diff = diff_runs(&a, &b, &options());
    let f = find(&diff.findings, EXPENSES);
    assert_eq!(f.category, Category::Missing);
    assert_eq!(f.t_first, 9);
    assert!(f.exemplar.b.is_none());
}

#[test]
fn a_one_sided_component_is_structural_and_the_intersection_is_still_diffed() {
    let a = base("a");
    let keep: Vec<&str> = a
        .schema
        .components
        .iter()
        .map(|c| c.id.as_str())
        .filter(|id| *id != RESERVE)
        .collect();
    let b = subset("subset", "b", &keep);

    let diff = diff_runs(&a, &b, &options());
    // Different component sets are *not* by themselves incomparable (Q6).
    assert_ne!(diff.verdict, Verdict::Incomparable);
    let mismatch = diff.emit_mismatch.as_ref().expect("an emit mismatch");
    assert_eq!(mismatch.a, "outputs");
    assert_eq!(mismatch.b, "list");
    assert_ne!(
        mismatch.a_component_set_digest,
        mismatch.b_component_set_digest
    );
    assert_eq!(diff.components.common, 12);
    assert_eq!(diff.structural.only_in_a.len(), 1);
    assert_eq!(diff.structural.only_in_a[0].component, RESERVE);
    // The intersection agrees, so the verdict is `matched` despite the set difference.
    assert_eq!(diff.verdict, Verdict::Matched);

    // `--require-same-emit` restores exit 2 for exactly this case.
    let mut opts = options();
    opts.require_same_emit = true;
    let strict = diff_runs(&a, &b, &opts);
    assert_eq!(strict.verdict, Verdict::Incomparable);
    assert_eq!(strict.verdict.exit_code(), 2);
}

#[test]
fn disjoint_modelpoints_are_reported_not_diffed() {
    let a = restricted("mp-a", "a", |key| key < "TA00013");
    let b = restricted("mp-b", "b", |key| key >= "TA00013");
    let diff = diff_runs(&a, &b, &options());
    assert_eq!(diff.verdict, Verdict::Incomparable);
    assert_eq!(diff.verdict.exit_code(), 2);
    assert!(diff.incomparable.iter().any(|w| w.contains("disjoint")));
    assert_eq!(diff.modelpoints.common, 0);
    assert_eq!(diff.cells.compared, 0);
}

#[test]
fn a_partially_overlapping_portfolio_diffs_the_overlap_and_names_the_rest() {
    let a = base("a");
    let b = restricted("mp-subset", "b", |key| key != "TA00007");
    let diff = diff_runs(&a, &b, &options());
    assert_eq!(diff.verdict, Verdict::Matched);
    assert_eq!(diff.modelpoints.common, 24);
    assert_eq!(diff.structural.modelpoints_only_in_a, vec!["TA00007"]);
    assert!(diff.structural.modelpoints_only_in_b.is_empty());
}

// ---------------------------------------------------------------------------
// filters and truncation
// ---------------------------------------------------------------------------

#[test]
fn component_and_modelpoint_filters_narrow_what_is_compared() {
    let (a, b) = inflated();

    let mut opts = options();
    opts.filter_component = Some("renewal_expenses".to_string());
    let one = diff_runs(&a, &b, &opts);
    assert_eq!(one.findings.len(), 1);
    assert_eq!(one.findings[0].component, EXPENSES);
    // Filtering hides findings; it does not change what the cell counts were computed over.
    assert!(one.cells.compared > 0);

    let mut opts = options();
    opts.filter_mp = Some("TA00003".to_string());
    let mp = diff_runs(&a, &b, &opts);
    assert!(mp
        .findings
        .iter()
        .flat_map(|f| f.cells.iter())
        .all(|c| c.mp_key == "TA00003"));
    assert_eq!(find(&mp.findings, EXPENSES).n_modelpoints, 1);
}

#[test]
fn top_truncates_and_says_how_much_it_hid() {
    let (a, b) = inflated();
    let mut opts = options();
    opts.top = 2;
    let diff = diff_runs(&a, &b, &opts);
    assert_eq!(diff.findings.len(), 2);
    assert_eq!(diff.root_divergences, 1);
    let (kept, of) = diff.findings_truncated.expect("truncation is recorded");
    assert_eq!(kept, 2);
    assert!(of > 2);
    assert_eq!(diff.to_json()["findings_truncated"]["of"], of);
    // The root survives truncation, because truncation happens after ranking.
    assert_eq!(diff.findings[0].component, EXPENSES);
}

// ---------------------------------------------------------------------------
// the JSON contract
// ---------------------------------------------------------------------------

#[test]
fn diff_json_is_the_pvf1_rundiff_document_of_5_4() {
    let (a, b) = inflated();
    let diff = diff_runs(&a, &b, &options());
    let json = diff.to_json();

    assert_eq!(json["format"], "pvf/1");
    assert_eq!(json["kind"], "rundiff");
    assert_eq!(json["a"]["system"], "predictable");
    assert_eq!(json["b"]["system"], "predictable");
    assert!(json["a"]["manifest"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    assert_eq!(json["summary"]["verdict"], "diverged");
    assert_eq!(json["summary"]["root_divergences"], 1);
    assert_eq!(json["summary"]["modelpoints"]["common"], 25);
    assert!(json["summary"]["cells"]["diverged"].as_u64().unwrap() > 0);

    let f = &json["findings"][0];
    assert_eq!(f["id"], "F001");
    assert_eq!(f["class"], "root");
    assert_eq!(f["category"], "value");
    assert_eq!(f["component"], EXPENSES);
    assert_eq!(f["t_first"], 3);
    assert!(f["exemplar"]["mp_key"].is_string());
    assert!(f["worst"]["abs"].as_f64().unwrap() > 0.0);
    assert!(f["message"].as_str().unwrap().contains("diverges at t=3"));
    // §5.5: the detectors ran, and the constant 1.028 ratio was recognised as `(1 + inflation)`.
    let hypotheses = f["hypotheses"].as_array().unwrap();
    let ratio = hypotheses
        .iter()
        .find(|h| h["code"] == "H0101")
        .expect("a constant ratio is a constant ratio");
    assert_eq!(ratio["confidence"], "high");
    assert_eq!(ratio["evidence"]["ratio"].as_f64().unwrap(), 1.028);
    assert_eq!(
        ratio["evidence"]["matches"]["name"],
        "(1 + expense_inflation)"
    );
    assert_eq!(
        ratio["evidence_support"]["held"],
        ratio["evidence_support"]["tested"]
    );
    assert!(f["explain_command"]
        .as_str()
        .unwrap()
        .starts_with("predictable explain"));
    // The float round trip is exact, which is what makes the JSON evidence rather than a summary.
    let exemplar_b = f["exemplar"]["b"].as_f64().unwrap();
    let cell = diff.findings[0]
        .cells
        .iter()
        .find(|c| c.t == 3 && c.mp_row == diff.findings[0].exemplar.mp_row)
        .unwrap();
    assert_eq!(exemplar_b, cell.b.as_ref().unwrap().as_f64().unwrap());
}

#[test]
fn the_terminal_render_says_what_the_json_says() {
    let (a, b) = inflated();
    let diff = diff_runs(&a, &b, &options());
    let text = predictable_rundiff::render::render(&diff);
    assert!(text.contains("DIVERGED"));
    assert!(text.contains("F001"));
    assert!(text.contains("root"));
    assert!(text.contains("renewal_expenses"));
    assert!(text.contains(&diff.findings[0].message));
    assert!(text.contains("UNEXPLAINED"));
}

// ---------------------------------------------------------------------------
// no model available
// ---------------------------------------------------------------------------

#[test]
fn without_a_model_the_classification_says_it_is_ordering_not_the_ir() {
    let (a, b) = inflated();
    let opts = DiffOptions::default(); // no models
    let diff = diff_runs(&a, &b, &opts);
    assert!(!diff.graph_available);
    assert!(!diff.model_attribution_available);
    let f = find(&diff.findings, EXPENSES);
    assert_eq!(f.class_basis, "no_graph_earliest_t");
    assert!(f.explained_by_model_change.is_none());
    assert!(predictable_rundiff::render::render(&diff).contains("no model graph"));
}
