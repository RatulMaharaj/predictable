//! The seeded-mutation harness of `04-verify.md` §9.2 — the project's primary quality metric.
//!
//! > *"Take a reconciled model, apply a catalogue of seeded mutations (drop a `-1`, flip a timing
//! > tag, change a `clamp` to `exact`, insert a `/12`), run the diff, and assert that (a) the
//! > correct component is reported as the root divergence, (b) the correct `t_first` is found, and
//! > (c) the intended hypothesis code fires. **This is the primary quality metric for the whole
//! > project**: `root-cause hit rate` over the mutation catalogue."*
//!
//! ## What this module is, and what it is not
//!
//! It is the catalogue and the scoring. It is deliberately **not** the runner: a mutation is
//! applied to a copy of a reference model's source, and *somebody else* runs the model twice and
//! hands back the two [`crate::RunDiff`]s. That split keeps this crate free of a dependency on the
//! engine while letting the harness be driven from a test, from an example binary, or from CI.
//!
//! ## Scoring is three independent questions
//!
//! A differ that names the right component but the wrong `t` is wrong in a different way from one
//! that names the right `t` in the wrong component, and averaging them into one number would hide
//! both. So [`Score`] keeps `component`, `t_first` and `hypothesis` apart, and `root_cause` — the
//! headline hit rate — is the conjunction of the first two: *the correct component reported as the
//! root divergence, at the correct `t`*.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::model::{Class, Verdict};
use crate::RunDiff;

/// One seeded mutation: a literal edit to one reference model, and what the diff must then say.
#[derive(Debug, Clone, PartialEq)]
pub struct Mutation {
    /// Stable id, printed in the hit-rate table.
    pub id: &'static str,
    /// The reference model directory under `models/`.
    pub model: &'static str,
    /// The file to edit, relative to the model directory.
    pub file: &'static str,
    /// The text to find. Must occur exactly once, or the harness refuses to apply it — a mutation
    /// that lands in two places is two mutations and the expectations would be a coin toss.
    pub find: &'static str,
    /// What to put there.
    pub replace: &'static str,
    /// What kind of mistake this imitates, for the published table.
    pub kind: Kind,
    /// The component whose divergence is the *root*, qualified as results qualify it.
    pub expect_component: &'static str,
    /// The earliest diverging `t` of that component. `None` when the mutation is not localised in
    /// time (a `PerMP` value has `t = -1` and that is asserted like any other).
    pub expect_t_first: Option<i32>,
    /// The hypothesis code the detectors are expected to fire on the root finding.
    pub expect_hypothesis: Option<&'static str>,
}

/// The class of mistake a mutation imitates. Named, because the published table is more useful
/// broken down by kind than as a single number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A lag dropped or added: `x[t-1]` became `x`.
    Lag,
    /// A timing tag flipped: `start` became `end`.
    Timing,
    /// A scale factor: a `/12`, a `× 1000`.
    Scale,
    /// An additive term appeared or vanished.
    Offset,
    /// A sign convention flipped.
    Sign,
    /// The Prophet `/12` rate-conversion idiom.
    RateConversion,
    /// A table lookup policy or a table's key range.
    TablePolicy,
    /// An indicator or term expiry off by one.
    Expiry,
    /// A rounding step that is not in the other model.
    Rounding,
}

impl Kind {
    /// The word the table prints.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Lag => "lag",
            Kind::Timing => "timing",
            Kind::Scale => "scale",
            Kind::Offset => "offset",
            Kind::Sign => "sign",
            Kind::RateConversion => "rate conversion",
            Kind::TablePolicy => "table policy",
            Kind::Expiry => "expiry",
            Kind::Rounding => "rounding",
        }
    }
}

/// The committed catalogue.
///
/// Every entry is a mistake somebody has actually made in a migration: the dropped lag, the
/// annual rate divided by twelve, the term test that includes its last anniversary, the report
/// written to two decimal places. The point of the catalogue is that it is *boring* — a differ
/// that only finds exotic divergences is a differ that finds nothing on a Tuesday.
pub fn catalogue() -> Vec<Mutation> {
    vec![
        Mutation {
            id: "TA-lag-dropped",
            model: "term_annual",
            file: "build/model.pir",
            find: "num_pols_if[t-1] * (1 - qx[t-1]) * (1 - wx[t-1])",
            replace: "num_pols_if[t-1] * (1 - qx) * (1 - wx[t-1])",
            kind: Kind::Lag,
            expect_component: "model.num_pols_if",
            expect_t_first: Some(1),
            expect_hypothesis: None,
        },
        Mutation {
            id: "TA-scale-twelfth",
            model: "term_annual",
            file: "build/model.pir",
            find: "renewal_expense_pa * compound(expense_inflation, t)",
            replace: "renewal_expense_pa / 12 * compound(expense_inflation, t)",
            kind: Kind::Scale,
            expect_component: "model.renewal_expenses",
            expect_t_first: Some(0),
            expect_hypothesis: Some("H0101"),
        },
        Mutation {
            id: "TA-offset-added",
            model: "term_annual",
            file: "build/model.pir",
            find: "pv_claims + pv_expenses + initial_expense - pv_premiums",
            replace: "pv_claims + pv_expenses + initial_expense - pv_premiums + 100.0",
            kind: Kind::Offset,
            expect_component: "model.bel",
            expect_t_first: Some(-1),
            expect_hypothesis: Some("H0102"),
        },
        Mutation {
            id: "TA-sign-flipped",
            model: "term_annual",
            file: "build/model.pir",
            find: "expr = \"-bel / pv_premiums\"",
            replace: "expr = \"bel / pv_premiums\"",
            kind: Kind::Sign,
            expect_component: "model.profit_margin",
            expect_t_first: Some(-1),
            expect_hypothesis: Some("H0103"),
        },
        Mutation {
            id: "TA-timing-flipped",
            model: "term_annual",
            file: "build/model.pir",
            find: "timing = \"start\"\nexpr = \"premium_rate * num_pols_if * in_force_factor\"",
            replace: "timing = \"end\"\nexpr = \"premium_rate * num_pols_if * in_force_factor\"",
            kind: Kind::Timing,
            expect_component: "model.pv_premiums",
            expect_t_first: Some(-1),
            expect_hypothesis: Some("H0202"),
        },
        Mutation {
            id: "TA-term-off-by-one",
            model: "term_annual",
            file: "build/model.pir",
            find: "expr = \"t < policy_term\"",
            replace: "expr = \"t <= policy_term\"",
            kind: Kind::Expiry,
            expect_component: "model.in_term",
            expect_t_first: Some(10),
            expect_hypothesis: Some("H0302"),
        },
        Mutation {
            id: "TA-rounded-premium",
            model: "term_annual",
            file: "build/model.pir",
            find: "expr = \"premium_rate * num_pols_if * in_force_factor\"",
            replace: "expr = \"round(premium_rate * num_pols_if * in_force_factor, 2)\"",
            kind: Kind::Rounding,
            expect_component: "model.premium_income",
            // `t = 0` is the issue premium, which the modelpoint file already carries at two
            // decimal places: the first period the rounding can bite is `t = 1`.
            expect_t_first: Some(1),
            expect_hypothesis: Some("H0402"),
        },
        Mutation {
            id: "TA-table-truncated",
            model: "term_annual",
            file: "tables/lapses.csv",
            find: "5,0.05\n6,0.046\n7,0.043\n8,0.041\n",
            replace: "",
            kind: Kind::TablePolicy,
            expect_component: "model.wx",
            // `policy_year = t + 1`, so the first missing key (5) is first read at `t = 4`.
            expect_t_first: Some(4),
            expect_hypothesis: Some("H0301"),
        },
        Mutation {
            id: "TM-prophet-twelfth-rate",
            model: "term_monthly",
            file: "build/model.pir",
            find: "expr = \"pow(1 + valuation_rate, 0.08333333333333333) - 1\"",
            replace: "expr = \"nominal_to_periodic(valuation_rate, 12)\"",
            kind: Kind::RateConversion,
            expect_component: "model.monthly_valuation_rate",
            expect_t_first: Some(-1),
            expect_hypothesis: Some("H0401"),
        },
        Mutation {
            id: "TM-prophet-twelfth-inflation",
            model: "term_monthly",
            file: "build/model.pir",
            find: "expr = \"pow(1 + expense_inflation, 0.08333333333333333) - 1\"",
            replace: "expr = \"nominal_to_periodic(expense_inflation, 12)\"",
            kind: Kind::RateConversion,
            expect_component: "model.monthly_expense_inflation",
            expect_t_first: Some(-1),
            expect_hypothesis: Some("H0401"),
        },
        Mutation {
            id: "TM-term-off-by-one",
            model: "term_monthly",
            file: "build/model.pir",
            find: "expr = \"t < policy_term * 12\"",
            replace: "expr = \"t <= policy_term * 12\"",
            kind: Kind::Expiry,
            expect_component: "model.in_term",
            expect_t_first: Some(120),
            expect_hypothesis: Some("H0302"),
        },
        Mutation {
            id: "TM-lag-dropped",
            model: "term_monthly",
            file: "build/model.pir",
            find: "num_pols_if[t-1] * (1 - qx[t-1]) * (1 - wx[t-1])",
            replace: "num_pols_if[t-1] * (1 - qx) * (1 - wx[t-1])",
            kind: Kind::Lag,
            expect_component: "model.num_pols_if",
            // Monthly: `qx` is stepped from an annual table on attained age, so `qx[t-1]` and
            // `qx[t]` agree until the first birthday. The differ has to find `t = 12`, and a
            // catalogue that expected `t = 1` would be asserting against the model, not the diff.
            expect_t_first: Some(12),
            expect_hypothesis: None,
        },
        Mutation {
            id: "SM-maturity-off-by-one",
            model: "savings_monthly",
            file: "build/model.pir",
            find: "expr = \"t == term_months - 1\"",
            replace: "expr = \"t == term_months\"",
            kind: Kind::Expiry,
            expect_component: "model.is_maturity",
            expect_t_first: Some(119),
            expect_hypothesis: None,
        },
        Mutation {
            id: "SM-amc-twelfth",
            model: "savings_monthly",
            file: "build/model.pir",
            find: "expr = \"1 - pow(1 - amc_pa, 0.08333333333333333)\"",
            replace: "expr = \"nominal_to_periodic(amc_pa, 12)\"",
            kind: Kind::Scale,
            expect_component: "model.amc_m",
            expect_t_first: Some(-1),
            expect_hypothesis: Some("H0101"),
        },
    ]
}

/// Apply a mutation to the text of the named file.
///
/// `Err` when `find` does not occur exactly once: an ambiguous mutation would make the expected
/// answer a matter of luck, and a silently-unapplied one would score as a differ failure when it
/// is a catalogue failure.
pub fn apply(mutation: &Mutation, text: &str) -> Result<String, String> {
    let hits = text.matches(mutation.find).count();
    if hits != 1 {
        return Err(format!(
            "{}: `find` occurs {hits} times in {}, expected exactly once",
            mutation.id, mutation.file
        ));
    }
    Ok(text.replacen(mutation.find, mutation.replace, 1))
}

/// Runs one model: given the `[run]` file and an output directory, produce a run directory.
///
/// The harness never runs anything itself. This is the seam — the test passes the CLI, CI could
/// pass a remote executor, and neither changes what the catalogue means.
pub type RunModel<'r> = dyn Fn(&Path, &Path) -> Result<(), String> + 'r;

/// Apply the whole catalogue and score it.
///
/// For each mutation: two pristine copies of the reference model are made, the mutation is applied
/// to one of them, both are run, and the two run directories are diffed. Two copies rather than
/// one edited in place, because the diff verifies each side's recorded model digest before it
/// classifies from it — side a's `.pir` has to still be side a's `.pir`.
///
/// Both sides are run with `emit = "all"`. That is not a thumb on the scale: it is the migration
/// mode of IR §8.4.2 and the only mode in which "which component is the root" is a question the
/// result set can answer at all. With `emit = "outputs"` the answer is always "the earliest output
/// downstream of the mistake", which is a fact about the emit setting, not about the differ.
pub fn run_catalogue(
    models_root: &Path,
    scratch: &Path,
    run: &RunModel<'_>,
) -> Result<HitRate, String> {
    let mut scores = Vec::new();
    for mutation in catalogue() {
        scores.push(run_one(models_root, scratch, run, &mutation)?);
    }
    Ok(HitRate::new(scores))
}

/// Apply one mutation, run both sides, diff, and score.
pub fn run_one(
    models_root: &Path,
    scratch: &Path,
    run: &RunModel<'_>,
    mutation: &Mutation,
) -> Result<Score, String> {
    let source = models_root.join(mutation.model);
    let base = scratch.join(mutation.id);
    let _ = std::fs::remove_dir_all(&base);
    let (dir_a, dir_b) = (base.join("a"), base.join("b"));
    copy_model(&source, &dir_a)?;
    copy_model(&source, &dir_b)?;

    let target = dir_b.join(mutation.file);
    let text = read(&target)?;
    write(&target, &apply(mutation, &text)?)?;
    if mutation.file.ends_with(".csv") {
        // A table's bytes are pinned by a digest in the module that declares it (§2.9.1). Editing
        // the CSV without restamping it would make the run fail to load, and the catalogue would
        // be measuring the loader instead of the differ.
        restamp_table_digests(&dir_b)?;
    }

    let mut out = Vec::new();
    for dir in [&dir_a, &dir_b] {
        // `emit = "all"`: see the note on `run_catalogue`.
        let run_file = dir.join("run.pir");
        let text = read(&run_file)?
            .replace("emit = \"outputs\"", "emit = \"all\"")
            // `emit = "all"` needs the full series retained: a ring buffer cannot emit what it
            // has already overwritten (IR §8.4.2).
            .replace("retain = \"ring\"", "retain = \"full\"");
        write(&run_file, &text)?;
        let target = dir.join("out");
        run(&run_file, &target)?;
        out.push(target);
    }

    let a = crate::RunSide::load("a", &out[0]).map_err(|e| e.to_string())?;
    let b = crate::RunSide::load("b", &out[1]).map_err(|e| e.to_string())?;
    let files_a = model_inputs(&dir_a);
    let files_b = model_inputs(&dir_b);
    let opts = crate::DiffOptions {
        // `regression` and not `reconcile`: both sides came out of the same engine, so any
        // difference beyond reassociation drift is the mutation, and a money-scale tolerance would
        // silently absorb the rounding mutation the catalogue is trying to measure.
        profile: crate::Profile::builtin("regression").expect("regression is a builtin"),
        top: 0,
        ..crate::DiffOptions::default()
    }
    .with_model_paths(Some(&files_a), Some(&files_b));
    let diff = crate::diff_runs(&a, &b, &opts);
    Ok(score(mutation, &diff))
}

/// The `.pir` inputs of a model copy: the built modules and the assumption set.
fn model_inputs(dir: &Path) -> Vec<PathBuf> {
    [
        "build/model.pir",
        "build/product.pir",
        "build/schema.pir",
        "base.pir",
    ]
    .iter()
    .map(|f| dir.join(f))
    .filter(|p| p.is_file())
    .collect()
}

/// Recompute every `[[table]]`'s `digest` from the file it points at.
fn restamp_table_digests(dir: &Path) -> Result<(), String> {
    for entry in walk(&dir.join("build"))? {
        if entry.extension().and_then(|e| e.to_str()) != Some("pir") {
            continue;
        }
        let text = read(&entry)?;
        let mut index = crate::SourceIndex::empty();
        let name = entry.display().to_string();
        index.add(&name, &text);
        let mut edits: Vec<crate::SuggestedEdit> = Vec::new();
        for table in table_names(&text) {
            let (Some(source), Some(digest)) = (
                index.table_field(&table, "source"),
                index.table_field(&table, "digest"),
            ) else {
                continue;
            };
            let Some(rel) = source.unquoted() else {
                continue;
            };
            let Ok(bytes) = std::fs::read(dir.join(rel)) else {
                continue;
            };
            let stamped = format!("sha256:{}", crate::sha256_hex(&bytes));
            edits.push(digest.replace(format!("\"{stamped}\""), "restamp"));
        }
        edits.sort_by_key(|e| std::cmp::Reverse(e.byte_start));
        let mut text = text;
        for edit in edits {
            text = edit
                .apply(&text)
                .ok_or_else(|| format!("{name}: digest edit no longer applies"))?;
        }
        write(&entry, &text)?;
    }
    Ok(())
}

/// The declared table names of a `.pir` file, in declaration order.
fn table_names(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_table = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_table = line == "[[table]]";
            continue;
        }
        if in_table {
            if let Some(rest) = line.strip_prefix("name = \"") {
                if let Some(name) = rest.strip_suffix('"') {
                    out.push(name.to_string());
                }
                in_table = false;
            }
        }
    }
    out
}

fn copy_model(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(from).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        // The committed run and the Python cache are not inputs to a run.
        if name == "runs" || name == "__pycache__" {
            continue;
        }
        let target = to.join(&name);
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            copy_model(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn walk(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.path().is_file() {
            out.push(entry.path());
        }
    }
    out.sort();
    Ok(out)
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// How one mutation scored.
#[derive(Debug, Clone, PartialEq)]
pub struct Score {
    /// The mutation's id.
    pub id: &'static str,
    /// Its kind.
    pub kind: Kind,
    /// The diff reported a divergence at all.
    pub diverged: bool,
    /// The expected component is reported, and reported as a *root* divergence.
    pub component: bool,
    /// Its `t_first` is the expected one.
    pub t_first: bool,
    /// The expected hypothesis code fired on that finding. `None` when the catalogue expects none.
    pub hypothesis: Option<bool>,
    /// What was actually reported as the root, for the failure message.
    pub reported: Vec<String>,
    /// The hypothesis codes that actually fired on the expected component.
    pub codes: Vec<String>,
}

impl Score {
    /// The headline: the right component, named as root, at the right `t`.
    pub fn root_cause(&self) -> bool {
        self.component && self.t_first
    }
}

/// Score one diff against the mutation that produced it.
pub fn score(mutation: &Mutation, diff: &RunDiff) -> Score {
    let expected = diff
        .findings
        .iter()
        .find(|f| f.component == mutation.expect_component);
    let component = expected.map(|f| f.class == Class::Root).unwrap_or(false);
    let t_first = match (expected, mutation.expect_t_first) {
        (Some(f), Some(t)) => f.t_first == t,
        (Some(_), None) => true,
        (None, _) => false,
    };
    let codes: Vec<String> = expected
        .map(|f| f.hypotheses.iter().map(|h| h.code.to_string()).collect())
        .unwrap_or_default();
    let hypothesis = mutation
        .expect_hypothesis
        .map(|code| codes.iter().any(|c| c == code));
    Score {
        id: mutation.id,
        kind: mutation.kind,
        diverged: diff.verdict == Verdict::Diverged,
        component,
        t_first,
        hypothesis,
        reported: diff
            .findings
            .iter()
            .filter(|f| f.class == Class::Root)
            .map(|f| format!("{}@t={}", f.component, f.t_first))
            .collect(),
        codes,
    }
}

/// The published metric: hit rates over a set of scores.
#[derive(Debug, Clone, PartialEq)]
pub struct HitRate {
    /// Every score, in catalogue order.
    pub scores: Vec<Score>,
}

impl HitRate {
    /// Build from scores.
    pub fn new(scores: Vec<Score>) -> HitRate {
        HitRate { scores }
    }

    /// How many mutations were scored.
    pub fn total(&self) -> usize {
        self.scores.len()
    }

    /// Mutations where the right component was named root at the right `t`.
    pub fn root_cause_hits(&self) -> usize {
        self.scores.iter().filter(|s| s.root_cause()).count()
    }

    /// `root-cause hit rate` — the number §9.2 calls the project's primary quality metric.
    pub fn root_cause_rate(&self) -> f64 {
        rate(self.root_cause_hits(), self.total())
    }

    /// Of the mutations whose catalogue entry names an expected hypothesis, the share where it
    /// fired. Reported separately because a hypothesis is a bonus on top of a located root, never
    /// a substitute for one.
    pub fn hypothesis_rate(&self) -> (usize, usize) {
        let of = self
            .scores
            .iter()
            .filter(|s| s.hypothesis.is_some())
            .count();
        let hit = self
            .scores
            .iter()
            .filter(|s| s.hypothesis == Some(true))
            .count();
        (hit, of)
    }

    /// The markdown table published in the docs. Deterministic: catalogue order, fixed columns.
    pub fn markdown(&self) -> String {
        let mut out = String::new();
        out.push_str("| mutation | kind | root component | `t_first` | hypothesis |\n");
        out.push_str("|---|---|---|---|---|\n");
        for s in &self.scores {
            let hyp = match s.hypothesis {
                Some(true) => "yes",
                Some(false) => "**no**",
                None => "—",
            };
            out.push_str(&format!(
                "| `{}` | {} | {} | {} | {} |\n",
                s.id,
                s.kind.word(),
                tick(s.component),
                tick(s.t_first),
                hyp
            ));
        }
        let (hit, of) = self.hypothesis_rate();
        out.push_str(&format!(
            "\n**Root-cause hit rate: {}/{} = {:.0}%.** Intended hypothesis fired on {hit}/{of}.\n",
            self.root_cause_hits(),
            self.total(),
            self.root_cause_rate() * 100.0,
        ));
        out
    }

    /// Per-kind breakdown, ascending by kind, for the docs page.
    pub fn by_kind(&self) -> Vec<(Kind, usize, usize)> {
        let kinds: BTreeSet<Kind> = self.scores.iter().map(|s| s.kind).collect();
        kinds
            .into_iter()
            .map(|k| {
                let of = self.scores.iter().filter(|s| s.kind == k).count();
                let hit = self
                    .scores
                    .iter()
                    .filter(|s| s.kind == k && s.root_cause())
                    .count();
                (k, hit, of)
            })
            .collect()
    }
}

fn rate(hit: usize, of: usize) -> f64 {
    if of == 0 {
        0.0
    } else {
        hit as f64 / of as f64
    }
}

fn tick(ok: bool) -> &'static str {
    if ok {
        "yes"
    } else {
        "**no**"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalogue_entry_has_a_distinct_id() {
        let ids: BTreeSet<&str> = catalogue().iter().map(|m| m.id).collect();
        assert_eq!(ids.len(), catalogue().len());
    }

    #[test]
    fn an_ambiguous_mutation_is_refused_rather_than_applied_to_the_first_match() {
        let m = &catalogue()[0];
        let doubled = format!("{}\n{}", m.find, m.find);
        assert!(apply(m, &doubled).is_err());
        assert!(apply(m, "nothing here").is_err());
        let once = format!("prefix {} suffix", m.find);
        assert_eq!(
            apply(m, &once).unwrap(),
            format!("prefix {} suffix", m.replace)
        );
    }

    #[test]
    fn the_hit_rate_table_is_deterministic_and_reports_both_metrics() {
        let s = |id: &'static str, comp: bool, t: bool, hyp: Option<bool>| Score {
            id,
            kind: Kind::Lag,
            diverged: true,
            component: comp,
            t_first: t,
            hypothesis: hyp,
            reported: Vec::new(),
            codes: Vec::new(),
        };
        let r = HitRate::new(vec![
            s("a", true, true, Some(true)),
            s("b", true, false, Some(false)),
            s("c", false, false, None),
        ]);
        assert_eq!(r.root_cause_hits(), 1);
        assert_eq!(r.hypothesis_rate(), (1, 2));
        let md = r.markdown();
        assert_eq!(md, HitRate::new(r.scores.clone()).markdown());
        assert!(md.contains("Root-cause hit rate: 1/3 = 33%"));
        assert_eq!(r.by_kind(), vec![(Kind::Lag, 1, 3)]);
    }
}
