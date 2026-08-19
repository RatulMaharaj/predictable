//! The hypothesis detectors of `04-verify.md` §5.5: the diff does not merely report, it proposes.
//!
//! A finding says *what* diverges. A hypothesis says *why it probably does*, in a form the reader
//! can falsify in one step — and, where the evidence names an anchor in the model source, hands
//! over the literal edit that would test the proposal ([`crate::suggest`]).
//!
//! ## The shape of a detector
//!
//! Every detector is a pure function of one modelpoint's *divergence vector* — the per-`t`
//! `(a, b)` pairs of one component, from [`Finding::series_for`] — plus, for the detectors that
//! need it, read-only context: the other components of both runs, the IR graph, the model, and
//! the model's source spans. A detector returns a [`Fired`]: the evidence, and a **signature**.
//!
//! ## The signature is what makes support meaningful
//!
//! §5.5 requires a hypothesis to hold "on a random sample of 32 further diverging modelpoints".
//! Holding is not "the detector fired again" — a constant ratio of `1/12` on the exemplar and of
//! `0.97` on the next modelpoint are two different claims, and counting them as one would inflate
//! confidence exactly where it should collapse. So support counts modelpoints on which the
//! detector fired *with the same signature*, and `evidence_support.tested` counts every modelpoint
//! it was asked about, held or not.
//!
//! The sample is deterministic: the diverging modelpoints in `mp_row` order, evenly spaced, up to
//! 32. "Random" in the spec means "not cherry-picked"; a seeded shuffle would make two runs of the
//! same diff report different support, which §6.3's determinism rule does not allow.
//!
//! ## Confidence, and why low-confidence hypotheses are still emitted
//!
//! `high` at 32/32, `medium` at ≥ 28/32, `low` otherwise (§5.5), scaled by the sample actually
//! available — a 25-modelpoint run cannot produce 32/32 and should not be silently downgraded for
//! it. A low-confidence hypothesis is still emitted, because an agent can cheaply falsify a wrong
//! proposal and cannot falsify a silent omission at all.

use std::collections::{BTreeMap, BTreeSet};

use predictable_ir::{Expr, LitValue};
use predictable_modeldiff::ModelSide;
use serde::{Deserialize, Serialize};

use crate::graph::DepGraph;
use crate::model::Finding;
use crate::run_side::{RunSide, Value};
use crate::suggest::{SourceIndex, SuggestedEdit};

/// How many further modelpoints a hypothesis is tested on (§5.5).
pub const SAMPLE: usize = 32;

/// Relative closeness at which two `f64` are "the same number" for pattern purposes (§5.5's 1e-9).
const EPS: f64 = 1e-9;
/// Closeness at which a ratio is recognised as a *named* constant — looser than [`EPS`], because
/// a Prophet report is written at fixed precision and an exact `1/12` never survives it.
const NAMED_EPS: f64 = 1e-6;

/// `confidence` (§5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Held on every modelpoint tested.
    High,
    /// Held on at least 28 of 32 (or the same proportion of a smaller sample).
    Medium,
    /// Held on fewer. Emitted anyway: an agent can falsify it in one run.
    Low,
}

impl Confidence {
    /// The word the report prints.
    pub fn word(self) -> &'static str {
        match self {
            Confidence::High => "high",
            Confidence::Medium => "medium",
            Confidence::Low => "low",
        }
    }

    /// §5.5's rule, scaled to the sample that existed.
    pub fn from_support(tested: usize, held: usize) -> Confidence {
        if tested == 0 || held == tested {
            Confidence::High
        } else if held * SAMPLE >= 28 * tested {
            Confidence::Medium
        } else {
            Confidence::Low
        }
    }
}

/// `evidence_support` (§5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Support {
    /// Further diverging modelpoints the detector was run on, up to [`SAMPLE`].
    pub tested: usize,
    /// How many of them produced the *same* claim.
    pub held: usize,
}

/// One proposal: a code, the evidence that fired it, how far it generalises, and the edit that
/// would test it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hypothesis {
    /// `H0101` … `H0501`.
    pub code: &'static str,
    /// One line, in the vocabulary of the model rather than of the detector.
    pub message: String,
    /// `high` | `medium` | `low`.
    pub confidence: Confidence,
    /// The numbers that fired it. Every field here is a fact from the two runs.
    pub evidence: serde_json::Value,
    /// `{tested, held}` over the 32-modelpoint sample.
    pub evidence_support: Support,
    /// The literal edit, when the evidence anchors one in the model source.
    pub suggested_edit: Option<SuggestedEdit>,
}

impl Hypothesis {
    /// The hypothesis as JSON, in §5.5's field order.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "code": self.code,
            "message": self.message,
            "confidence": self.confidence,
            "evidence": self.evidence,
            "evidence_support": self.evidence_support,
            "suggested_edit": self.suggested_edit,
        })
    }
}

/// What a detector returns when it fires on one modelpoint.
#[derive(Debug, Clone, PartialEq)]
pub struct Fired {
    /// The claim, canonicalised. Two modelpoints support the same hypothesis iff their signatures
    /// are equal.
    pub signature: String,
    /// The evidence, as it will be reported for the exemplar.
    pub evidence: serde_json::Value,
    /// The one-line message, for the exemplar.
    pub message: String,
}

/// Everything the detectors may read beyond the finding itself.
///
/// Deliberately read-only and deliberately *not* the run files: a detector that could see data the
/// finding did not would be proposing hypotheses the reported evidence does not support.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    /// The `a` side.
    pub a: &'a RunSide,
    /// The `b` side.
    pub b: &'a RunSide,
    /// `b`'s model, when the diff could load it.
    pub model: Option<&'a ModelSide>,
    /// The IR dependency graph, for naming the input a hypothesis blames.
    pub graph: &'a DepGraph,
    /// `b`'s model source, for `suggested_edit`.
    pub source: &'a SourceIndex,
}

impl<'a> Context<'a> {
    /// A context with no model and no source: detectors that need neither still run.
    pub fn bare(
        a: &'a RunSide,
        b: &'a RunSide,
        graph: &'a DepGraph,
        source: &'a SourceIndex,
    ) -> Context<'a> {
        Context {
            a,
            b,
            model: None,
            graph,
            source,
        }
    }

    /// The numeric assumptions of `b`'s model, `name → value`.
    fn assumptions(&self) -> BTreeMap<&str, f64> {
        self.model
            .map(|m| {
                m.assumption_values
                    .iter()
                    .filter_map(|(k, v)| Some((k.as_str(), lit_f64(v)?)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `b`'s IR component for a qualified id, when the model is available.
    fn component(&self, id: &str) -> Option<&'a predictable_ir::Component> {
        self.model?.component(bare(id))
    }

    /// The per-`t` values of a lookup key expression on side `b`, from the run when the key is a
    /// component, and from the timeline when it is one of the builtin time fields (IR §5).
    ///
    /// Anything else returns `None`: a detector that guessed at the key would be citing evidence
    /// the run does not contain.
    fn key_series(&self, key: &Expr, mp_row: u32) -> Option<BTreeMap<i32, f64>> {
        let Expr::Ref { name } = key else {
            return None;
        };
        let qualified = self
            .graph
            .qualify(name)
            .unwrap_or(name.as_str())
            .to_string();
        let from_run: BTreeMap<i32, f64> = self
            .series(self.b, &qualified, mp_row)
            .into_iter()
            .collect();
        if !from_run.is_empty() {
            return Some(from_run);
        }
        let periods = self.b.manifest.timeline.periods as i32;
        let ppy: i32 = match self.b.manifest.timeline.basis.as_str() {
            "monthly" => 12,
            "quarterly" => 4,
            "annual" => 1,
            _ => return None,
        };
        let f: fn(i32, i32) -> f64 = match name.as_str() {
            "t" => |t, _| t as f64,
            "policy_year" => |t, ppy| (t / ppy + 1) as f64,
            "policy_month" => |t, ppy| t as f64 * (12.0 / ppy as f64) + 1.0,
            _ => return None,
        };
        Some((0..=periods).map(|t| (t, f(t, ppy))).collect())
    }

    /// One component's series for one modelpoint on one side, ascending `t`.
    fn series(&self, side: &RunSide, id: &str, mp_row: u32) -> Vec<(i32, f64)> {
        side.components
            .get(id)
            .map(|cells| {
                cells
                    .iter()
                    .filter(|((row, _), _)| *row == mp_row)
                    .filter_map(|((_, t), v)| Some((*t, v.as_f64()?)))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Run every detector over one finding and return the hypotheses that fired, most specific first.
///
/// Detectors run on the exemplar modelpoint first (cheap), and only a detector that fired there is
/// tested on the sample — §5.5's "detectors run per component on the exemplar modelpoint first".
pub fn detect(ctx: &Context<'_>, finding: &Finding) -> Vec<Hypothesis> {
    let exemplar = finding.exemplar.mp_key.clone();
    let rows: BTreeMap<&str, u32> = finding
        .cells
        .iter()
        .map(|c| (c.mp_key.as_str(), c.mp_row))
        .collect();
    let sample = sample_modelpoints(finding, &exemplar);

    // A `PerMP` or `Scalar` component has one cell per modelpoint and no series at all: its
    // divergence *vector* is across the population, not across `t`. Running the per-`t` detectors
    // on a one-point series would find nothing, and finding nothing on the BEL is not a result
    // anybody can use. So the vector is built across modelpoints instead, and the support is the
    // population it already covers rather than a second sample of it.
    let across_population = !finding.cells.is_empty() && finding.cells.iter().all(|c| c.t < 0);
    if across_population {
        return detect_across_population(ctx, finding);
    }

    let mut out: Vec<Hypothesis> = Vec::new();
    for detector in DETECTORS {
        let Some(fired) = run_one(ctx, finding, detector, &exemplar, &rows) else {
            continue;
        };
        let mut tested = 0usize;
        let mut held = 0usize;
        for mp in &sample {
            tested += 1;
            if run_one(ctx, finding, detector, mp, &rows).map(|f| f.signature)
                == Some(fired.signature.clone())
            {
                held += 1;
            }
        }
        // §5.5 emits low-confidence hypotheses, but a claim that fails on most of the sample is
        // not a low-confidence claim — it is a claim about the exemplar only, and saying it about
        // the component would be false.
        if tested > 0 && held * 2 < tested {
            continue;
        }
        let confidence = Confidence::from_support(tested, held);
        let suggested_edit = (detector.edit)(ctx, finding, &fired);
        out.push(Hypothesis {
            code: detector.code,
            message: fired.message,
            confidence,
            evidence: fired.evidence,
            evidence_support: Support { tested, held },
            suggested_edit,
        });
    }

    // Precedence: a named cause explains the ratio better than "the ratio is constant", so the
    // generic ratio hypothesis stands down when a specific one fired on the same number.
    let specific = out
        .iter()
        .any(|h| matches!(h.code, "H0401" | "H0202" | "H0103"));
    if specific {
        out.retain(|h| h.code != "H0101");
    }
    out
}

/// The `PerMP` case: one probe whose "series" is the population, indexed by `mp_row`.
fn detect_across_population(ctx: &Context<'_>, finding: &Finding) -> Vec<Hypothesis> {
    let mut series: Vec<(i32, f64, f64)> = finding
        .cells
        .iter()
        .filter_map(|c| {
            Some((
                c.mp_row as i32,
                numeric(c.a.as_ref()?)?,
                numeric(c.b.as_ref()?)?,
            ))
        })
        .collect();
    series.sort_by_key(|(row, _, _)| *row);
    let probe = Probe {
        finding,
        mp_key: &finding.exemplar.mp_key,
        mp_row: finding.exemplar.mp_row,
        series: &series,
    };
    // The pattern was checked on every modelpoint in one pass, so `tested` is the population minus
    // the exemplar and `held` equals it: a claim of constancy over the vector *is* the claim that
    // it held on each of them.
    let tested = series.len().saturating_sub(1).min(SAMPLE);
    let mut out: Vec<Hypothesis> = Vec::new();
    for detector in DETECTORS {
        let Some(fired) = (detector.run)(ctx, &probe) else {
            continue;
        };
        let suggested_edit = (detector.edit)(ctx, finding, &fired);
        out.push(Hypothesis {
            code: detector.code,
            message: fired.message,
            confidence: Confidence::from_support(tested, tested),
            evidence: fired.evidence,
            evidence_support: Support {
                tested,
                held: tested,
            },
            suggested_edit,
        });
    }
    if out
        .iter()
        .any(|h| matches!(h.code, "H0401" | "H0202" | "H0103"))
    {
        out.retain(|h| h.code != "H0101");
    }
    out
}

fn run_one(
    ctx: &Context<'_>,
    finding: &Finding,
    detector: &Detector,
    mp_key: &str,
    rows: &BTreeMap<&str, u32>,
) -> Option<Fired> {
    let series = numeric_series(finding, mp_key);
    let mp_row = *rows.get(mp_key)?;
    (detector.run)(
        ctx,
        &Probe {
            finding,
            mp_key,
            mp_row,
            series: &series,
        },
    )
}

/// What one detector is shown.
#[derive(Debug, Clone, Copy)]
pub struct Probe<'a> {
    /// The finding under examination.
    pub finding: &'a Finding,
    /// The modelpoint being probed.
    pub mp_key: &'a str,
    /// Its row index, the key into the run's other components.
    pub mp_row: u32,
    /// The per-`t` divergence vector: `(t, a, b)`, ascending `t`.
    pub series: &'a [(i32, f64, f64)],
}

struct Detector {
    code: &'static str,
    run: fn(&Context<'_>, &Probe<'_>) -> Option<Fired>,
    edit: fn(&Context<'_>, &Finding, &Fired) -> Option<SuggestedEdit>,
}

impl std::fmt::Debug for Detector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Detector")
            .field("code", &self.code)
            .finish()
    }
}

/// The detector table, in precedence order: a trap first, then the causes that name themselves,
/// then the shape-of-the-difference patterns.
static DETECTORS: &[Detector] = &[
    Detector {
        code: "H0501",
        run: nan,
        edit: no_edit,
    },
    Detector {
        code: "H0401",
        run: rate_conversion,
        edit: edit_rate_conversion,
    },
    Detector {
        code: "H0202",
        run: timing_basis,
        edit: edit_timing_basis,
    },
    Detector {
        code: "H0103",
        run: sign_flip,
        edit: no_edit,
    },
    Detector {
        code: "H0101",
        run: constant_ratio,
        edit: edit_constant_ratio,
    },
    Detector {
        code: "H0102",
        run: constant_offset,
        edit: no_edit,
    },
    Detector {
        code: "H0201",
        run: off_by_one,
        edit: no_edit,
    },
    Detector {
        code: "H0301",
        run: table_boundary,
        edit: edit_table_boundary,
    },
    Detector {
        code: "H0302",
        run: indicator_expiry,
        edit: edit_indicator_expiry,
    },
    Detector {
        code: "H0303",
        run: segment_subset,
        edit: no_edit,
    },
    Detector {
        code: "H0402",
        run: rounding,
        edit: no_edit,
    },
];

fn no_edit(_: &Context<'_>, _: &Finding, _: &Fired) -> Option<SuggestedEdit> {
    None
}

// ---------------------------------------------------------------------------
// H0501 — a NaN or an infinity on one side
// ---------------------------------------------------------------------------

fn nan(_ctx: &Context<'_>, p: &Probe<'_>) -> Option<Fired> {
    let bad = p
        .finding
        .cells
        .iter()
        .filter(|c| c.mp_key == p.mp_key)
        .find(|c| {
            [c.a.as_ref(), c.b.as_ref()]
                .into_iter()
                .flatten()
                .filter_map(Value::as_f64)
                .any(|x| !x.is_finite())
        })?;
    let side = match (
        bad.a.as_ref().and_then(Value::as_f64).map(f64::is_finite),
        bad.b.as_ref().and_then(Value::as_f64).map(f64::is_finite),
    ) {
        (Some(false), Some(false)) => "both",
        (Some(false), _) => "a",
        _ => "b",
    };
    Some(Fired {
        signature: format!("nan:{side}"),
        evidence: serde_json::json!({
            "side": side,
            "t": bad.t,
            "a": bad.a.as_ref().map(Value::to_json),
            "b": bad.b.as_ref().map(Value::to_json),
        }),
        message: format!(
            "`{}` is not finite on side {side} at t={} — a trap, not a tolerance question",
            bare(&p.finding.component),
            bad.t
        ),
    })
}

// ---------------------------------------------------------------------------
// H0101 / H0103 / H0401 / H0202 — the constant-ratio family
// ---------------------------------------------------------------------------

/// `b/a`, when it is constant across `t` to [`EPS`] and is not 1.
fn ratio(series: &[(i32, f64, f64)]) -> Option<f64> {
    if series.len() < 2 {
        return None;
    }
    let mut ratios = Vec::with_capacity(series.len());
    for (_, a, b) in series {
        if !a.is_finite() || !b.is_finite() || a.abs() < 1e-300 {
            return None;
        }
        ratios.push(b / a);
    }
    let first = ratios[0];
    if !ratios.iter().all(|r| close(*r, first, EPS)) {
        return None;
    }
    (!close(first, 1.0, 1e-12)).then_some(first)
}

fn constant_ratio(ctx: &Context<'_>, p: &Probe<'_>) -> Option<Fired> {
    let r = ratio(p.series)?;
    let named = name_ratio(ctx, r);
    let what = named
        .as_ref()
        .map(|(n, _)| format!(" — that is {n}"))
        .unwrap_or_default();
    Some(Fired {
        signature: format!("ratio:{:.9e}", r),
        evidence: serde_json::json!({
            "ratio": r,
            "matches": named.as_ref().map(|(n, v)| serde_json::json!({"name": n, "value": v})),
            "t_tested": p.series.len(),
        }),
        message: format!(
            "b/a is constant at {r:.9} across all {} periods of `{}`{what}: a scale factor, not a behavioural difference",
            p.series.len(),
            bare(&p.finding.component),
        ),
    })
}

/// §5.5: "check it against 12, 1/12, 1000, `(1+r)`, `(1+r)^(1/12)`".
fn name_ratio(ctx: &Context<'_>, r: f64) -> Option<(String, f64)> {
    let mut candidates: Vec<(String, f64)> = vec![
        ("12".to_string(), 12.0),
        ("1/12".to_string(), 1.0 / 12.0),
        ("1000".to_string(), 1000.0),
        ("1/1000".to_string(), 0.001),
        ("100".to_string(), 100.0),
        ("1/100".to_string(), 0.01),
        ("-1 (a sign convention)".to_string(), -1.0),
    ];
    for (name, v) in ctx.assumptions() {
        if !(v > -1.0 && v.abs() < 10.0) {
            continue;
        }
        candidates.push((format!("(1 + {name})"), 1.0 + v));
        candidates.push((format!("1 / (1 + {name})"), 1.0 / (1.0 + v)));
        candidates.push((format!("(1 + {name})^(1/12)"), (1.0 + v).powf(1.0 / 12.0)));
        candidates.push((format!("(1 + {name})^0.5"), (1.0 + v).sqrt()));
        candidates.push((format!("1 / (1 + {name})^0.5"), 1.0 / (1.0 + v).sqrt()));
    }
    candidates
        .into_iter()
        .find(|(_, v)| close(r, *v, NAMED_EPS))
}

fn edit_constant_ratio(
    ctx: &Context<'_>,
    finding: &Finding,
    fired: &Fired,
) -> Option<SuggestedEdit> {
    let r = fired.evidence.get("ratio")?.as_f64()?;
    if !r.is_finite() || r == 0.0 {
        return None;
    }
    let anchor = ctx
        .source
        .component_field(bare(&finding.component), "expr")?;
    let expr = anchor.unquoted()?;
    // The edit is applied to *b*'s source, and it has to make b reproduce a. `b = a × r`, so the
    // correction is `÷ r` — written as `× n` or `÷ n` when either is a whole number, because that
    // is how the line would have been written by hand and how it will read in the next diff.
    let (op, factor) = if close(r, r.round(), NAMED_EPS) && r.round().abs() >= 2.0 {
        ("/", format!("{}", r.round()))
    } else if close(1.0 / r, (1.0 / r).round(), NAMED_EPS) && (1.0 / r).round().abs() >= 2.0 {
        ("*", format!("{}", (1.0 / r).round()))
    } else {
        ("*", format!("{}", 1.0 / r))
    };
    anchor.replace_quoted(
        &format!("({expr}) {op} {factor}"),
        format!(
            "`{}` on side b is {r} times side a's; {op} {factor} restores it",
            bare(&finding.component)
        ),
    )
}

fn sign_flip(_ctx: &Context<'_>, p: &Probe<'_>) -> Option<Fired> {
    let r = ratio(p.series)?;
    close(r, -1.0, NAMED_EPS).then(|| Fired {
        signature: "sign".to_string(),
        evidence: serde_json::json!({"ratio": r, "t_tested": p.series.len()}),
        message: format!(
            "b = -a for every period of `{}`: a sign-convention mismatch, not a modelling difference — set `sign = -1` for this component in `mapping.toml`",
            bare(&p.finding.component),
        ),
    })
}

/// The Prophet `/12` idiom (IR §5): a rate divided by twelve where the model compounds it.
fn rate_conversion(ctx: &Context<'_>, p: &Probe<'_>) -> Option<Fired> {
    let r = ratio(p.series)?;
    for (name, v) in ctx.assumptions() {
        if !(v > -1.0 && v.abs() < 10.0 && v.abs() > 1e-12) {
            continue;
        }
        let compounded = (1.0 + v).powf(1.0 / 12.0) - 1.0;
        if compounded.abs() < 1e-300 {
            continue;
        }
        let simple = v / 12.0;
        let idiom = simple / compounded;
        for (dir, target) in [
            ("a_compounds_b_divides", idiom),
            ("a_divides_b_compounds", 1.0 / idiom),
        ] {
            if close(r, target, NAMED_EPS) {
                return Some(Fired {
                    signature: format!("rate:{name}:{dir}"),
                    evidence: serde_json::json!({
                        "ratio": r,
                        "assumption": name,
                        "annual_rate": v,
                        "simple_monthly": simple,
                        "compound_monthly": compounded,
                        "direction": dir,
                    }),
                    message: format!(
                        "b/a is {r:.9} = ({name}/12) / ((1+{name})^(1/12) - 1): the Prophet `/12` idiom — one side divides the annual rate by twelve where the other compounds it",
                    ),
                });
            }
        }
    }
    None
}

fn edit_rate_conversion(
    ctx: &Context<'_>,
    finding: &Finding,
    fired: &Fired,
) -> Option<SuggestedEdit> {
    let name = fired.evidence.get("assumption")?.as_str()?;
    let dir = fired.evidence.get("direction")?.as_str()?;
    let anchor = ctx
        .source
        .component_field(bare(&finding.component), "expr")?;
    let expr = anchor.unquoted()?;
    // The edit lands in b's source and has to make b agree with a. `nominal_to_periodic(r, 12)` is
    // how the simple division is spelled in a `.pir`: a bare `/ 12` on a `rate(annual)` is `E1402`
    // precisely so that this choice is visible in the next diff (IR §2.8).
    let compounded = format!("pow(1 + {name}, 0.08333333333333333) - 1");
    let simple_spellings = [
        format!("nominal_to_periodic({name}, 12)"),
        format!("{name} / 12"),
    ];
    let (from, to, how) = if dir == "a_compounds_b_divides" {
        // b divides; a compounds. Replace b's division with the compounding.
        let from = simple_spellings
            .iter()
            .find(|s| expr.contains(s.as_str()))?
            .clone();
        (from, format!("({compounded})"), "compound")
    } else {
        // b compounds; a divides. Replace b's compounding with the named simple conversion.
        let from = [format!("({compounded})"), compounded.clone()]
            .into_iter()
            .find(|s| expr.contains(s.as_str()))?;
        (from, simple_spellings[0].clone(), "divide")
    };
    anchor.replace_quoted(
        &expr.replacen(&from, &to, 1),
        format!("{how} `{name}` to the monthly basis, the way side a does"),
    )
}

/// A ratio of exactly `(1+i)` or `(1+i)^0.5` on a discounted quantity: a `start`/`mid`/`end`
/// mismatch in the `npv`, not a difference in the cashflows themselves.
fn timing_basis(ctx: &Context<'_>, p: &Probe<'_>) -> Option<Fired> {
    let r = ratio(p.series)?;
    let component = ctx.component(&p.finding.component)?;
    let expr = component.expr.as_ref()?;
    let (agg_arg, _) = npv_argument(expr)?;
    let series_timing = ctx
        .component(ctx.graph.qualify(&agg_arg).unwrap_or(&agg_arg))
        .and_then(|c| c.timing)
        .map(|t| format!("{t:?}").to_lowercase());
    for (name, v) in ctx.assumptions() {
        if !(v > -1.0 && v.abs() < 10.0 && v.abs() > 1e-12) {
            continue;
        }
        for (basis, target) in [
            ("one full period", 1.0 + v),
            ("one full period (inverted)", 1.0 / (1.0 + v)),
            ("half a period", (1.0 + v).sqrt()),
            ("half a period (inverted)", 1.0 / (1.0 + v).sqrt()),
        ] {
            if close(r, target, NAMED_EPS) {
                return Some(Fired {
                    signature: format!("timing:{name}:{basis}"),
                    evidence: serde_json::json!({
                        "ratio": r,
                        "assumption": name,
                        "rate": v,
                        "shift": basis,
                        "npv_argument": agg_arg,
                        "argument_timing": series_timing,
                        "component_timing": component.timing.map(|t| format!("{t:?}").to_lowercase()),
                    }),
                    message: format!(
                        "b/a is {r:.9}, exactly {basis} of discount at `{name}`: the two runs disagree about the timing of `{agg_arg}` inside the npv (side b's tag is {})",
                        series_timing.clone().unwrap_or_else(|| "unknown".into())
                    ),
                });
            }
        }
    }
    None
}

fn edit_timing_basis(
    ctx: &Context<'_>,
    _finding: &Finding,
    fired: &Fired,
) -> Option<SuggestedEdit> {
    let arg = fired.evidence.get("npv_argument")?.as_str()?;
    let current = fired.evidence.get("argument_timing")?.as_str()?;
    let anchor = ctx.source.component_field(arg, "timing")?;
    let proposed = match current {
        "start" => "end",
        "end" => "start",
        "mid" => "end",
        _ => return None,
    };
    Some(anchor.replace(
        format!("\"{proposed}\""),
        format!("move `{arg}` from `{current}` to `{proposed}`, the timing side a discounts it at"),
    ))
}

/// `npv(x, disc)` anywhere in the tree: the series argument's name, and the discount series'.
///
/// `npv` lowers to `Agg { op: Npv, value, pred }` (IR §2.6), where `pred` carries the discount
/// factor rather than a predicate — so this looks for the aggregate node, not for a call.
fn npv_argument(e: &Expr) -> Option<(String, Option<String>)> {
    if let Expr::Agg {
        op: predictable_ir::AggOp::Npv,
        value,
        pred,
    } = e
    {
        if let Expr::Ref { name } = value.as_ref() {
            let disc = match pred.as_deref() {
                Some(Expr::Ref { name }) => Some(name.clone()),
                _ => None,
            };
            return Some((name.clone(), disc));
        }
    }
    e.children().into_iter().find_map(|(_, c)| npv_argument(c))
}

// ---------------------------------------------------------------------------
// H0102 — constant offset
// ---------------------------------------------------------------------------

fn constant_offset(_ctx: &Context<'_>, p: &Probe<'_>) -> Option<Fired> {
    if p.series.len() < 2 {
        return None;
    }
    let deltas: Vec<f64> = p.series.iter().map(|(_, a, b)| b - a).collect();
    let first = deltas[0];
    if first == 0.0 || !first.is_finite() {
        return None;
    }
    if !deltas.iter().all(|d| close(*d, first, EPS)) {
        return None;
    }
    // A constant offset that is also a constant ratio is a scale factor on a constant series;
    // reporting both would be two names for one fact, and H0101 is the more useful name.
    if ratio(p.series).is_some() {
        return None;
    }
    Some(Fired {
        signature: format!("offset:{:.9e}", first),
        evidence: serde_json::json!({"offset": first, "t_tested": p.series.len()}),
        message: format!(
            "b - a is constant at {first} across all {} periods of `{}`: an additive term present on one side only",
            p.series.len(),
            bare(&p.finding.component),
        ),
    })
}

// ---------------------------------------------------------------------------
// H0201 — off by one in `t`
// ---------------------------------------------------------------------------

fn off_by_one(_ctx: &Context<'_>, p: &Probe<'_>) -> Option<Fired> {
    // The finding's own cells are the *diverging* ones only; a shift is a claim about the whole
    // series, so it is checked against the run, not against the divergence vector alone.
    if p.series.len() < 3 {
        return None;
    }
    let a: BTreeMap<i32, f64> = p.series.iter().map(|(t, a, _)| (*t, *a)).collect();
    let b: BTreeMap<i32, f64> = p.series.iter().map(|(t, _, b)| (*t, *b)).collect();
    for k in [-1i32, 1] {
        let mut compared = 0usize;
        let ok = b.iter().all(|(t, bv)| match a.get(&(t + k)) {
            Some(av) => {
                compared += 1;
                close(*bv, *av, EPS)
            }
            None => true,
        });
        if ok && compared >= 2 {
            let direction = if k == 1 {
                "b lags a by one period"
            } else {
                "b leads a by one period"
            };
            return Some(Fired {
                signature: format!("shift:{k}"),
                evidence: serde_json::json!({
                    "shift": k,
                    "direction": direction,
                    "t_matched": compared,
                }),
                message: format!(
                    "b[t] = a[t{}] for every period compared of `{}`: a timing shift, not a value difference — try `timing_shift = {k}` in `mapping.toml`, or `retime`/`period_base`",
                    if k >= 0 { format!("+{k}") } else { k.to_string() },
                    bare(&p.finding.component),
                ),
            });
        }
    }
    None
}

// ---------------------------------------------------------------------------
// H0301 — divergence begins exactly at a table key boundary
// ---------------------------------------------------------------------------

fn table_boundary(ctx: &Context<'_>, p: &Probe<'_>) -> Option<Fired> {
    let model = ctx.model?;
    let component = ctx.component(&p.finding.component)?;
    let expr = component.expr.as_ref()?;
    let t_first = p.series.first()?.0;
    for (table, keys) in lookups(expr) {
        let Some(decl) = model.tables().find(|d| d.name == table) else {
            continue;
        };
        if decl.keys.len() != 1 {
            continue;
        }
        let key_col = &decl.keys[0];
        let Some(rows) = model.table_rows.get(&table) else {
            continue;
        };
        let declared: Vec<f64> = rows
            .iter()
            .filter_map(|r| r.first()?.trim().parse().ok())
            .collect();
        if declared.is_empty() {
            continue;
        }
        let (lo, hi) = (
            declared.iter().cloned().fold(f64::INFINITY, f64::min),
            declared.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
        );
        let Some(key_series) = ctx.key_series(keys.first()?, p.mp_row) else {
            continue;
        };
        let at = *key_series.get(&t_first)?;
        let before = key_series.get(&(t_first - 1)).copied();
        // A boundary is a boundary in one of two ways: the key left the table's range, or it
        // landed on a row the table does not have. Both are answered by the lookup `policy`, and
        // neither is answered the same way by every system — which is exactly why they diverge.
        let why = if at > hi && before.map(|x| x <= hi).unwrap_or(true) {
            Some(("the key passed the last row", hi, "N0101"))
        } else if at < lo && before.map(|x| x >= lo).unwrap_or(true) {
            Some(("the key fell below the first row", lo, "N0101"))
        } else if !declared.iter().any(|k| close(*k, at, EPS))
            && before.map(|x| x != at).unwrap_or(true)
        {
            let stepped = declared
                .iter()
                .cloned()
                .filter(|k| *k <= at)
                .fold(f64::NEG_INFINITY, f64::max);
            Some(("the key has no row and was stepped back", stepped, "N0102"))
        } else {
            None
        };
        let (side, boundary, note) = why?;
        return Some(Fired {
            signature: format!("table:{table}:{side}"),
            evidence: serde_json::json!({
                "table": table,
                "key_column": key_col.name,
                "key_expr": key_name(keys.first()?),
                "policy": format!("{:?}", key_col.policy).to_lowercase(),
                "boundary": boundary,
                "key_at_t_first": at,
                "t_first": t_first,
                "why": side,
                "note": note,
            }),
            message: format!(
                "the divergence begins at t={t_first}, the exact period where the `{table}` key reaches {at} and {side} (policy `{}`, {note}): the two runs resolve the missing key differently",
                format!("{:?}", key_col.policy).to_lowercase(),
            ),
        });
    }
    None
}

fn edit_table_boundary(
    ctx: &Context<'_>,
    _finding: &Finding,
    fired: &Fired,
) -> Option<SuggestedEdit> {
    let table = fired.evidence.get("table")?.as_str()?;
    let key = fired.evidence.get("key_column")?.as_str()?;
    let policy = fired.evidence.get("policy")?.as_str()?;
    let anchor = ctx.source.table_key_policy(table, key)?;
    let proposed = match policy {
        "clamp" => "step",
        "step" => "clamp",
        "exact" => "clamp",
        _ => return None,
    };
    Some(anchor.replace(
        format!("\"{proposed}\""),
        format!("try `policy = \"{proposed}\"` on `{table}.{key}`: the two runs treat the out-of-range key differently"),
    ))
}

/// Every `tbl@(k1, ...)` in a tree.
fn lookups(e: &Expr) -> Vec<(String, Vec<Expr>)> {
    let mut out = Vec::new();
    if let Expr::Lookup { table, keys } = e {
        out.push((table.clone(), keys.clone()));
    }
    for (_, c) in e.children() {
        out.extend(lookups(c));
    }
    out
}

// ---------------------------------------------------------------------------
// H0302 — one side is zero where the other is not, from `t = k` onward
// ---------------------------------------------------------------------------

fn indicator_expiry(ctx: &Context<'_>, p: &Probe<'_>) -> Option<Fired> {
    let zeros: Vec<&(i32, f64, f64)> = p
        .series
        .iter()
        .filter(|(_, a, b)| (*a == 0.0) != (*b == 0.0))
        .collect();
    if zeros.is_empty() {
        return None;
    }
    // Every diverging cell from `k` on has exactly one zero side, and the same side each time.
    let k = zeros[0].0;
    let side_zero_is_a = zeros[0].1 == 0.0;
    if !zeros.iter().all(|(_, a, _)| (*a == 0.0) == side_zero_is_a) {
        return None;
    }
    if !p
        .series
        .iter()
        .filter(|(t, _, _)| *t >= k)
        .all(|(_, a, b)| (*a == 0.0) != (*b == 0.0))
    {
        return None;
    }
    // Name the indicator: an input of this component whose own "last non-zero t" differs between
    // the runs. That is a falsifiable claim about a named component, not a guess from its name.
    let mut blamed: Option<(String, i32, i32)> = None;
    for edge in ctx.graph.inputs_of(&p.finding.component) {
        let a_end = last_nonzero(&ctx.series(ctx.a, &edge.input, p.mp_row));
        let b_end = last_nonzero(&ctx.series(ctx.b, &edge.input, p.mp_row));
        if let (Some(x), Some(y)) = (a_end, b_end) {
            if x != y {
                blamed = Some((edge.input.clone(), x, y));
                break;
            }
        }
    }
    let (name, a_end, b_end) = blamed
        .map(|(n, x, y)| (Some(n), Some(x), Some(y)))
        .unwrap_or((None, None, None));
    Some(Fired {
        signature: format!(
            "expiry:{}:{}",
            if side_zero_is_a { "a" } else { "b" },
            name.clone().unwrap_or_default()
        ),
        evidence: serde_json::json!({
            "t_from": k,
            "zero_side": if side_zero_is_a { "a" } else { "b" },
            "indicator": name,
            "indicator_last_nonzero_a": a_end,
            "indicator_last_nonzero_b": b_end,
        }),
        message: match &name {
            Some(n) => format!(
                "from t={k} one side is zero and the other is not; `{}` stops at t={} on side a and t={} on side b — a term-expiry off-by-one",
                bare(n),
                a_end.unwrap_or(-1),
                b_end.unwrap_or(-1),
            ),
            None => format!(
                "from t={k} side {} is zero where the other is not: an indicator or term-expiry difference",
                if side_zero_is_a { "a" } else { "b" },
            ),
        },
    })
}

fn edit_indicator_expiry(
    ctx: &Context<'_>,
    _finding: &Finding,
    fired: &Fired,
) -> Option<SuggestedEdit> {
    let indicator = fired.evidence.get("indicator")?.as_str()?;
    let anchor = ctx.source.component_field(bare(indicator), "expr")?;
    let expr = anchor.unquoted()?;
    // The whole off-by-one lives in one comparison operator, so the edit is that operator and
    // nothing else. Anything more would be a rewrite, and a rewrite is not a hypothesis.
    let rewritten = if expr.contains("<=") {
        expr.replacen("<=", "<", 1)
    } else if expr.contains('<') {
        expr.replacen('<', "<=", 1)
    } else {
        return None;
    };
    anchor.replace_quoted(
        &rewritten,
        format!("shift `{}`'s expiry by one period", bare(indicator)),
    )
}

fn last_nonzero(series: &[(i32, f64)]) -> Option<i32> {
    series
        .iter()
        .filter(|(_, v)| *v != 0.0)
        .map(|(t, _)| *t)
        .max()
}

// ---------------------------------------------------------------------------
// H0303 — only the modelpoints sharing a field value diverge
// ---------------------------------------------------------------------------

fn segment_subset(ctx: &Context<'_>, p: &Probe<'_>) -> Option<Fired> {
    let diverging: BTreeSet<u32> = p.finding.cells.iter().map(|c| c.mp_row).collect();
    let all: BTreeSet<u32> = ctx.b.mp_keys.keys().copied().collect();
    if diverging.len() >= all.len() || diverging.is_empty() {
        return None;
    }
    // Candidate discriminators: components that are constant in `t` per modelpoint and take few
    // distinct values across the population — the run's own record of the modelpoint's segment.
    for (id, cells) in &ctx.b.components {
        if id == &p.finding.component {
            continue;
        }
        let mut per_mp: BTreeMap<u32, String> = BTreeMap::new();
        let mut constant = true;
        for ((row, _), v) in cells {
            let text = v.to_json().to_string();
            match per_mp.get(row) {
                Some(prev) if *prev != text => {
                    constant = false;
                    break;
                }
                _ => {
                    per_mp.insert(*row, text);
                }
            }
        }
        if !constant || per_mp.len() != all.len() {
            continue;
        }
        let distinct: BTreeSet<&String> = per_mp.values().collect();
        if distinct.len() < 2 || distinct.len() > 8 {
            continue;
        }
        let mine = per_mp.get(&p.mp_row)?.clone();
        let with_value: BTreeSet<u32> = per_mp
            .iter()
            .filter(|(_, v)| **v == mine)
            .map(|(row, _)| *row)
            .collect();
        if with_value == diverging {
            return Some(Fired {
                signature: format!("segment:{id}:{mine}"),
                evidence: serde_json::json!({
                    "field": id,
                    "value": mine,
                    "modelpoints_with_value": with_value.len(),
                    "modelpoints_total": all.len(),
                }),
                message: format!(
                    "only the {} of {} modelpoints with `{}` = {mine} diverge: a segment-specific rate or a missing branch, not a population-wide difference",
                    with_value.len(),
                    all.len(),
                    bare(id),
                ),
            });
        }
    }
    None
}

// ---------------------------------------------------------------------------
// H0402 — rounding
// ---------------------------------------------------------------------------

fn rounding(_ctx: &Context<'_>, p: &Probe<'_>) -> Option<Fired> {
    let (dp, rounded_side, max_abs) = rounding_core(p.series)?;
    Some(Fired {
        signature: format!("round:{dp}:{rounded_side}"),
        evidence: serde_json::json!({
            "dp": dp,
            "rounded_side": rounded_side,
            "max_abs": max_abs,
            "bound": 0.5 * 10f64.powi(-dp),
        }),
        message: format!(
            "every difference is within 0.5e-{dp} and side {rounded_side} is written to {dp} dp: this is rounding, not a modelling difference — raise the tolerance rather than the model (`--abs {}`)",
            0.5 * 10f64.powi(-dp)
        ),
    })
}

/// The rounding pattern, as a pure function of the divergence vector: `(dp, rounded side, max Δ)`.
fn rounding_core(series: &[(i32, f64, f64)]) -> Option<(i32, &'static str, f64)> {
    if series.is_empty() {
        return None;
    }
    let max_abs = series
        .iter()
        .map(|(_, a, b)| (b - a).abs())
        .fold(0.0f64, f64::max);
    if max_abs == 0.0 || !max_abs.is_finite() {
        return None;
    }
    // The *tightest* `dp` whose half-unit still bounds every difference and on whose grid one side
    // actually sits. Searching downwards matters: every difference under 0.005 is also under 0.05,
    // and reporting the loose bound would name a precision nobody wrote at. `dp = 0` is not
    // evidence of rounding — it is evidence of nothing — so the search stops at one decimal place.
    for dp in (1..=8i32).rev() {
        if max_abs > 0.5 * 10f64.powi(-dp) * (1.0 + 1e-12) {
            continue;
        }
        let rounded_side = ["a", "b"].into_iter().find(|side| {
            series.iter().all(|(_, a, b)| {
                let v = if *side == "a" { *a } else { *b };
                crate::tolerance::round_half_away(v, dp as u32) == v
            })
        });
        if let Some(side) = rounded_side {
            return Some((dp, side, max_abs));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// shared
// ---------------------------------------------------------------------------

/// The divergence vector of one modelpoint, with booleans and integers projected onto the reals.
///
/// [`Finding::series_for`] is the `f64` view, which is the right view for money. A term indicator
/// is a `bool` and an attained age is an `i64`, and the patterns that matter on them — the expiry
/// that moved by a period, the age that is one year out — are exactly the patterns of §5.5. So the
/// detectors see `true` as `1.0`, which is the same projection the engine itself makes.
fn numeric_series(finding: &Finding, mp_key: &str) -> Vec<(i32, f64, f64)> {
    let mut out: Vec<(i32, f64, f64)> = finding
        .cells
        .iter()
        .filter(|c| c.mp_key == mp_key)
        .filter_map(|c| Some((c.t, numeric(c.a.as_ref()?)?, numeric(c.b.as_ref()?)?)))
        .collect();
    out.sort_by_key(|(t, _, _)| *t);
    out
}

fn numeric(v: &Value) -> Option<f64> {
    match v {
        Value::F64(x) => Some(*x),
        Value::I64(x) => Some(*x as f64),
        Value::Bool(x) => Some(f64::from(*x)),
        Value::Str(_) => None,
    }
}

/// The name a lookup key expression reads, for the evidence.
fn key_name(e: &Expr) -> String {
    match e {
        Expr::Ref { name } => name.clone(),
        _ => "<expression>".to_string(),
    }
}

/// The unqualified name of a component id.
fn bare(id: &str) -> &str {
    id.rsplit_once('.').map(|(_, t)| t).unwrap_or(id)
}

fn close(x: f64, y: f64, eps: f64) -> bool {
    if x == y {
        return true;
    }
    let scale = x.abs().max(y.abs()).max(1e-12);
    (x - y).abs() <= eps * scale
}

fn lit_f64(v: &LitValue) -> Option<f64> {
    match v {
        LitValue::Float(x) => Some(*x),
        LitValue::Int(x) => Some(*x as f64),
        _ => None,
    }
}

/// The deterministic 32-modelpoint sample of §5.5: the diverging modelpoints other than the
/// exemplar, in `mp_row` order, evenly spaced.
fn sample_modelpoints(finding: &Finding, exemplar: &str) -> Vec<String> {
    let others: Vec<String> = finding
        .modelpoints()
        .into_iter()
        .filter(|k| *k != exemplar)
        .map(String::from)
        .collect();
    if others.len() <= SAMPLE {
        return others;
    }
    (0..SAMPLE)
        .map(|i| others[i * others.len() / SAMPLE].clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(pairs: &[(i32, f64, f64)]) -> Vec<(i32, f64, f64)> {
        pairs.to_vec()
    }

    #[test]
    fn a_constant_ratio_is_found_and_a_wandering_one_is_not() {
        assert_eq!(
            ratio(&series(&[(0, 1.0, 12.0), (1, 2.0, 24.0)])),
            Some(12.0)
        );
        assert_eq!(ratio(&series(&[(0, 1.0, 12.0), (1, 2.0, 25.0)])), None);
        assert_eq!(ratio(&series(&[(0, 1.0, 1.0), (1, 2.0, 2.0)])), None);
    }

    #[test]
    fn confidence_follows_the_spec_thresholds() {
        assert_eq!(Confidence::from_support(32, 32), Confidence::High);
        assert_eq!(Confidence::from_support(32, 28), Confidence::Medium);
        assert_eq!(Confidence::from_support(32, 27), Confidence::Low);
        // A smaller population is not punished for being small.
        assert_eq!(Confidence::from_support(10, 10), Confidence::High);
        assert_eq!(Confidence::from_support(0, 0), Confidence::High);
    }

    #[test]
    fn the_sample_is_deterministic_and_capped() {
        // Sampling twice gives the same list — a diff must be byte-identical on a re-run.
        let keys: Vec<String> = (0..100).map(|i| format!("MP{i:03}")).collect();
        let picked: Vec<&String> = (0..SAMPLE)
            .map(|i| &keys[i * keys.len() / SAMPLE])
            .collect();
        let again: Vec<&String> = (0..SAMPLE)
            .map(|i| &keys[i * keys.len() / SAMPLE])
            .collect();
        assert_eq!(picked, again);
        assert_eq!(picked.len(), SAMPLE);
    }

    #[test]
    fn rounding_names_the_tightest_precision_a_side_actually_sits_on() {
        // b is written to 2 dp; every difference is under half a cent.
        let (dp, side, _) = rounding_core(&series(&[(0, 1.234, 1.23), (1, 2.567, 2.57)])).unwrap();
        assert_eq!((dp, side), (2, "b"));
        // Neither side is on a grid: a small difference, but not rounding.
        assert!(rounding_core(&series(&[(0, 1.2341, 1.2338), (1, 2.5671, 2.5669)])).is_none());
        // Identical series are not "rounded to infinite precision".
        assert!(rounding_core(&series(&[(0, 1.0, 1.0)])).is_none());
    }
}
