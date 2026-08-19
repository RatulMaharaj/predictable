//! Per-`t` term retention for `explain()` (`01-ir.md` §11.2, decision Q11).
//!
//! Q11 is normative and blunt: *"the recording evaluator **must** retain per-`t`
//! `terms` for every `Agg` node, with the applied discount factor and
//! `timing_used` for `npv`, summing exactly to the node's value. Replay-only,
//! zero hot-path cost."* A `bel` that is 3% wrong tells you nothing; a `terms`
//! array whose discount exponent is `v^t` where it should be `v^(t+1)` tells you
//! everything at a glance.
//!
//! Two words in that ruling drive the implementation:
//!
//! * **Exactly.** The recorder walks `t = 0..=T` in the same order, with the
//!   same additions, as [`crate::reduce`] — it does not re-add the terms in a
//!   different order afterwards. `contribution` is therefore the exact addend
//!   the kernel used, and the running sum reproduces the kernel's value
//!   bit-for-bit. [`AggTrace::sums_exactly`] asserts it, and the test suite
//!   asserts it against the kernel's own output on adversarial magnitudes.
//! * **Replay-only.** Nothing in this module is reachable from the hot loop.
//!   The kernel's `Op::Reduce` / `Op::Npv` path is untouched; a trace is
//!   produced *after* a chunk has run, from the buffers it left behind, by
//!   [`crate::Engine::agg_trace`]. Tracing costs a run that does not ask for it
//!   nothing at all.
//!
//! Truncation is loud, never silent: `--trace-max-terms` (default
//! [`DEFAULT_MAX_TERMS`]) keeps the first and last `N/2` terms, sets
//! `terms_truncated`, and always reports the untruncated `term_count`.

use predictable_ir::{AggOp, Timing};
use serde::{Deserialize, Serialize};

/// `--trace-max-terms` default (`01-ir.md` §11.2).
pub const DEFAULT_MAX_TERMS: usize = 4096;

/// One period's contribution to an aggregate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Term {
    pub t: u32,
    /// The series value at `t`, before any discounting.
    pub value: f64,
    /// `npv` only: the discount factor *after* the timing exponent of §2.5 has
    /// been applied. The exponent is the bug; it must be visible as data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disc: Option<f64>,
    /// What was actually added to the running total — `0.0` for an excluded
    /// term, so the contributions always sum to the value.
    pub contribution: f64,
    /// Predicated aggregates only: did the predicate admit this period?
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub included: bool,
    /// `count_while` only: the first period whose value was false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stopped_here: bool,
}

fn is_true(b: &bool) -> bool {
    *b
}

/// The serde default for `included`: a term omitted from the JSON was admitted.
fn yes() -> bool {
    true
}

impl Term {
    fn plain(t: u32, value: f64, contribution: f64) -> Term {
        Term {
            t,
            value,
            disc: None,
            contribution,
            included: true,
            stopped_here: false,
        }
    }
}

/// The `Agg` node of an `explain()` trace (`01-ir.md` §11.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AggTrace {
    /// Always `"Agg"` — the trace's closed node-variant tag.
    pub node: &'static str,
    /// `sum`, `npv`, `last`, …
    pub op: String,
    /// The series being reduced.
    #[serde(rename = "ref")]
    pub reference: String,
    /// `npv` only: the timing tag that chose the discount exponent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing_used: Option<Timing>,
    /// The aggregate's value — bit-identical to what the kernel computed.
    pub value: f64,
    /// Periods in the untruncated reduction.
    pub term_count: usize,
    pub terms: Vec<Term>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub terms_truncated: bool,
}

impl AggTrace {
    /// Re-add the retained contributions left to right and compare with
    /// `value`. True for every untruncated additive trace; the recorder's
    /// contract in one assertion.
    pub fn sums_exactly(&self) -> bool {
        if self.terms_truncated {
            return true;
        }
        match self.op.as_str() {
            "sum" | "npv" | "count_while" => {
                let mut acc = 0.0;
                for term in &self.terms {
                    acc += term.contribution;
                }
                acc == self.value
            }
            // `first`/`last`/`max_over`/`min_over` select rather than add; the
            // selected term carries the value as its contribution.
            _ => self
                .terms
                .iter()
                .any(|term| term.contribution == self.value && term.included),
        }
    }

    /// Apply `--trace-max-terms`: keep the first and last `max/2`, and say so.
    pub(crate) fn truncate(mut self, max: usize) -> AggTrace {
        if max == 0 || self.terms.len() <= max {
            return self;
        }
        let half = max / 2;
        let tail_start = self.terms.len() - (max - half);
        let mut kept: Vec<Term> = self.terms[..half].to_vec();
        kept.extend_from_slice(&self.terms[tail_start..]);
        self.terms = kept;
        self.terms_truncated = true;
        self
    }
}

/// The recorded twin of [`crate::reduce::reduce`]: the same walk over
/// `t = 0..=t_max`, the same additions, plus a term per period.
///
/// `get` and `keep` are the caller's readers so this module never touches a
/// buffer layout; `crate::run` supplies them from the live chunk.
pub(crate) fn record_reduce(
    agg: AggOp,
    t_max: u32,
    get: &mut dyn FnMut(u32) -> f64,
    keep: &mut dyn FnMut(u32) -> bool,
    predicated: bool,
) -> (f64, Vec<Term>) {
    let mut terms = Vec::with_capacity(t_max as usize + 1);
    let value = match agg {
        AggOp::Sum => {
            let mut acc = 0.0;
            for t in 0..=t_max {
                let included = keep(t);
                let x = get(t);
                let contribution = if included { x } else { 0.0 };
                if included {
                    acc += x;
                }
                terms.push(Term {
                    included: !predicated || included,
                    ..Term::plain(t, x, contribution)
                });
            }
            acc
        }
        AggOp::SumKahan => {
            let (mut acc, mut c) = (0.0f64, 0.0f64);
            for t in 0..=t_max {
                let included = keep(t);
                let x = get(t);
                if included {
                    let sum = acc + x;
                    c += if acc.abs() >= x.abs() {
                        (acc - sum) + x
                    } else {
                        (x - sum) + acc
                    };
                    acc = sum;
                }
                terms.push(Term {
                    included: !predicated || included,
                    ..Term::plain(t, x, if included { x } else { 0.0 })
                });
            }
            acc + c
        }
        AggOp::First | AggOp::At | AggOp::Last | AggOp::MaxOver | AggOp::MinOver => {
            let mut chosen: Option<u32> = None;
            let mut value = match agg {
                AggOp::MaxOver => f64::NEG_INFINITY,
                AggOp::MinOver => f64::INFINITY,
                _ => 0.0,
            };
            for t in 0..=t_max {
                let included = keep(t);
                let x = get(t);
                if included {
                    let take = match agg {
                        AggOp::First | AggOp::At => chosen.is_none(),
                        AggOp::Last => true,
                        AggOp::MaxOver => x > value,
                        AggOp::MinOver => x < value,
                        _ => false,
                    };
                    if take {
                        value = x;
                        chosen = Some(t);
                    }
                }
                terms.push(Term {
                    included: !predicated || included,
                    ..Term::plain(t, x, 0.0)
                });
            }
            if let Some(t) = chosen {
                terms[t as usize].contribution = value;
            }
            value
        }
        // Stops at the first false (§2.8); the stopping period is marked.
        AggOp::CountWhile => {
            let mut n = 0.0;
            for t in 0..=t_max {
                let x = get(t);
                if !crate::ops::truthy(x) {
                    terms.push(Term {
                        included: false,
                        stopped_here: true,
                        ..Term::plain(t, x, 0.0)
                    });
                    break;
                }
                n += 1.0;
                terms.push(Term::plain(t, x, 1.0));
            }
            n
        }
        AggOp::Npv => 0.0,
    };
    (value, terms)
}

/// The recorded twin of [`crate::reduce::npv`]. `disc_at` returns the timing-
/// adjusted factor the kernel used at `t`, so the exponent bug shows up as data.
pub(crate) fn record_npv(
    t_max: u32,
    get: &mut dyn FnMut(u32) -> f64,
    disc_at: &mut dyn FnMut(u32) -> f64,
) -> (f64, Vec<Term>) {
    let mut acc = 0.0;
    let mut terms = Vec::with_capacity(t_max as usize + 1);
    for t in 0..=t_max {
        let x = get(t);
        let factor = disc_at(t);
        let contribution = x * factor;
        acc += contribution;
        terms.push(Term {
            t,
            value: x,
            disc: Some(factor),
            contribution,
            included: true,
            stopped_here: false,
        });
    }
    (acc, terms)
}

/// The `01-ir.md` §2.8 spelling of an aggregate, for the trace's `op` field.
pub(crate) fn agg_name(agg: AggOp) -> &'static str {
    match agg {
        AggOp::Sum => "sum",
        AggOp::SumKahan => "sum_kahan",
        AggOp::First => "first",
        AggOp::Last => "last",
        AggOp::At => "at",
        AggOp::MaxOver => "max_over",
        AggOp::MinOver => "min_over",
        AggOp::CountWhile => "count_while",
        AggOp::Npv => "npv",
    }
}
