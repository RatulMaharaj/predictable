//! Tolerance: the comparison predicate, the named profiles, and the override bookkeeping.
//!
//! `04-verify.md` §5.6: *"Comparison is defined once, here, and nowhere else."* Two `f64` values
//! match iff
//!
//! ```text
//! |a - b| <= abs_tol   OR   |a - b| <= rel_tol * max(|a|, |b|)
//! ```
//!
//! Disjunctive, so `abs_tol` covers values near zero and `rel_tol` covers large ones; and
//! `max(|a|,|b|)` rather than `|a|`, so `diff A B` and `diff B A` report identically. Every
//! deviation from the profile's own numbers — a per-unit rule, a per-component rule, a tolerance
//! raised by the source's own reporting precision — is recorded in [`ToleranceReport`] and printed.
//! A loosened tolerance must never be invisible.

use std::collections::BTreeMap;

use predictable_ir::Unit;
use serde::{Deserialize, Serialize};

use crate::error::DiffError;

/// An `(abs, rel)` pair.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Tol {
    /// Absolute tolerance.
    pub abs: f64,
    /// Relative tolerance, against `max(|a|, |b|)`.
    pub rel: f64,
}

impl Tol {
    /// A new pair.
    pub const fn new(abs: f64, rel: f64) -> Tol {
        Tol { abs, rel }
    }

    /// True when `self` permits at least as much difference as `other` in either dimension.
    pub fn is_looser_than(&self, other: &Tol) -> bool {
        self.abs > other.abs || self.rel > other.rel
    }
}

/// The comparison predicate of §5.6. The single definition; nothing else compares two floats.
///
/// ```
/// use predictable_rundiff::tolerance::{matches, Tol};
/// let t = Tol::new(0.005, 1e-6);
/// assert!(matches(10.0, 10.004, t));            // within abs
/// assert!(matches(1e9, 1e9 + 100.0, t));        // within rel
/// assert!(!matches(10.0, 10.31, t));
/// assert!(matches(-0.0, 0.0, Tol::new(0.0, 0.0)));   // signed zero is zero
/// assert!(!matches(f64::NAN, f64::NAN, Tol::new(1e9, 1.0)));  // NaN matches nothing
/// assert!(matches(f64::INFINITY, f64::INFINITY, Tol::new(0.0, 0.0)));
/// assert!(!matches(f64::INFINITY, f64::NEG_INFINITY, Tol::new(1e9, 1.0)));
/// ```
pub fn matches(a: f64, b: f64, tol: Tol) -> bool {
    // NaN never matches anything, including NaN (H0501): a trap is not a tolerance question.
    if a.is_nan() || b.is_nan() {
        return false;
    }
    if a.is_infinite() || b.is_infinite() {
        // +Inf matches +Inf only.
        return a == b;
    }
    // -0.0 == 0.0 in IEEE, so the difference is exactly 0 and the predicate already holds.
    let d = (a - b).abs();
    d <= tol.abs || d <= tol.rel * a.abs().max(b.abs())
}

/// The relative difference used for reporting, `|a-b| / max(|a|,|b|)`; `0.0` when both are zero.
pub fn rel_diff(a: f64, b: f64) -> f64 {
    let scale = a.abs().max(b.abs());
    if scale == 0.0 || !scale.is_finite() {
        return 0.0;
    }
    (a - b).abs() / scale
}

/// Which rule decided a component's tolerance. `--explain-tolerance` prints this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rule {
    /// The profile's own `abs`/`rel`.
    Profile,
    /// `[tolerance.<p>.by_unit]`, keyed by the unit name.
    Unit(String),
    /// `[tolerance.<p>.by_component]`, keyed by the component id.
    Component(String),
    /// §5.6 rounding rule 4: raised to `0.5 x 10^-dp` by the source's own reporting precision.
    SourcePrecision(u32),
}

impl Rule {
    /// A one-line spelling for the terminal.
    pub fn describe(&self) -> String {
        match self {
            Rule::Profile => "profile".to_string(),
            Rule::Unit(u) => format!("by_unit[{u}]"),
            Rule::Component(c) => format!("by_component[{c}]"),
            Rule::SourcePrecision(dp) => format!("source precision {dp}dp"),
        }
    }
}

/// A named, versioned tolerance profile (§5.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    /// The profile name, or `custom`.
    pub name: String,
    /// The profile's base tolerance.
    pub base: Tol,
    /// Display rounding for money in the terminal render. Never applied before comparison.
    pub money_dp: u32,
    /// `[tolerance.<p>.by_unit]`, keyed by the IR unit's name.
    pub by_unit: BTreeMap<String, Tol>,
    /// `[tolerance.<p>.by_component]`, keyed by qualified component id or bare name.
    pub by_component: BTreeMap<String, Tol>,
}

/// The names §5.6 defines.
pub const BUILTIN_PROFILES: &[&str] = &["exact", "regression", "reconcile", "materiality"];

impl Profile {
    /// One of the four named profiles of §5.6.
    pub fn builtin(name: &str) -> Result<Profile, DiffError> {
        let base = match name {
            "exact" => Tol::new(0.0, 0.0),
            "regression" => Tol::new(1e-9, 1e-12),
            "reconcile" => Tol::new(0.005, 1e-6),
            "materiality" => Tol::new(0.01, 1e-4),
            _ => {
                return Err(DiffError::UnknownProfile {
                    name: name.to_string(),
                    known: BUILTIN_PROFILES.iter().map(|s| (*s).to_string()).collect(),
                })
            }
        };
        let mut p = Profile {
            name: name.to_string(),
            base,
            money_dp: 2,
            by_unit: BTreeMap::new(),
            by_component: BTreeMap::new(),
        };
        // A half-cent tolerance on a probability is nonsense, so `reconcile` ships the per-unit
        // table of §5.6 rather than making every user write it out.
        if name == "reconcile" {
            p.by_unit.insert("prob".into(), Tol::new(1e-9, 1e-9));
            p.by_unit.insert("factor".into(), Tol::new(1e-10, 1e-10));
            p.by_unit.insert("count".into(), Tol::new(1e-6, 1e-9));
            p.by_unit.insert("money".into(), Tol::new(0.005, 1e-6));
        }
        Ok(p)
    }

    /// `--abs A --rel R`: a profile with no name but the same bookkeeping.
    pub fn custom(abs: f64, rel: f64) -> Profile {
        Profile {
            name: "custom".to_string(),
            base: Tol::new(abs, rel),
            money_dp: 2,
            by_unit: BTreeMap::new(),
            by_component: BTreeMap::new(),
        }
    }

    /// Add a per-unit override.
    pub fn with_unit(mut self, unit: &str, tol: Tol) -> Profile {
        self.by_unit.insert(unit.to_string(), tol);
        self
    }

    /// Add a per-component override.
    pub fn with_component(mut self, component: &str, tol: Tol) -> Profile {
        self.by_component.insert(component.to_string(), tol);
        self
    }

    /// Resolve the tolerance for one component.
    ///
    /// Precedence: per-component, then the source-precision raise, then per-unit, then the
    /// profile. The source-precision raise *raises* — it takes the max of the two `abs` values, so
    /// naming a component explicitly is never silently undone by the reader's opinion of the file.
    pub fn resolve(
        &self,
        component: &str,
        unit: Option<&Unit>,
        source: Option<SourceTol>,
    ) -> Resolved {
        let bare = component
            .rsplit_once('.')
            .map(|(_, t)| t)
            .unwrap_or(component);
        if let Some(t) = self
            .by_component
            .get(component)
            .or_else(|| self.by_component.get(bare))
        {
            return Resolved {
                tol: *t,
                rule: Rule::Component(component.to_string()),
            };
        }
        let unit_name = unit.map(unit_name).unwrap_or_default();
        // `rate(annual)` is keyable either in full or as the bare family name `rate`.
        let family = unit_name
            .split_once('(')
            .map(|(head, _)| head.to_string())
            .unwrap_or_else(|| unit_name.clone());
        let from_unit = self
            .by_unit
            .get(&unit_name)
            .or_else(|| self.by_unit.get(&family))
            .copied();
        let base = from_unit.unwrap_or(self.base);
        let rule = if from_unit.is_some() {
            Rule::Unit(unit_name)
        } else {
            Rule::Profile
        };
        match source {
            Some(s) if s.abs > base.abs => Resolved {
                tol: Tol::new(s.abs, base.rel),
                rule: Rule::SourcePrecision(s.dp),
            },
            _ => Resolved { tol: base, rule },
        }
    }
}

/// What the non-predictable side's own reporting precision permits (§5.6 rounding rule 4).
///
/// `abs` is not simply `0.5 x 10^-dp`: a column reported in thousands is reported to 2dp *of
/// thousands*, so the mapping's `scale` is part of what the file's precision means. Carrying the
/// scale here rather than at the comparison keeps the whole tolerance decision in one place.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceTol {
    /// Decimal places observed in the source.
    pub dp: u32,
    /// The absolute tolerance they imply, in the compared units.
    pub abs: f64,
}

impl SourceTol {
    /// `dp` decimals in units scaled by `scale`.
    pub fn new(dp: u32, scale: f64) -> SourceTol {
        SourceTol {
            dp,
            abs: 0.5 * 10f64.powi(-(dp as i32)) * scale.abs().max(f64::MIN_POSITIVE),
        }
    }
}

/// The tolerance in force for one component, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    /// The numbers.
    pub tol: Tol,
    /// The rule that produced them.
    pub rule: Rule,
}

/// One entry of `tolerance.overrides_applied`: a component whose tolerance is not the profile's.
#[derive(Debug, Clone, PartialEq)]
pub struct OverrideApplied {
    /// The component the rule fired for.
    pub component: String,
    /// The rule.
    pub rule: Rule,
    /// The tolerance it produced.
    pub tol: Tol,
    /// True when this permits *more* difference than the profile — the case that must be printed.
    pub looser: bool,
    /// Set for [`Rule::SourcePrecision`]: the note §5.6 rule 4 requires.
    pub note: Option<String>,
}

impl OverrideApplied {
    /// The `overrides_applied` entry as JSON.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "component": self.component,
            "rule": self.rule.describe(),
            "abs": self.tol.abs,
            "rel": self.tol.rel,
            "looser": self.looser,
            "note": self.note,
        })
    }
}

/// The tolerance section of `diff.json`, accumulated as components are compared.
#[derive(Debug, Clone, PartialEq)]
pub struct ToleranceReport {
    /// The profile in force.
    pub profile: Profile,
    /// Every non-profile rule that actually fired, sorted by component.
    pub overrides_applied: Vec<OverrideApplied>,
    /// Per-component resolutions, for `--explain-tolerance`.
    pub resolutions: BTreeMap<String, Resolved>,
}

impl ToleranceReport {
    /// An empty report against `profile`.
    pub fn new(profile: Profile) -> ToleranceReport {
        ToleranceReport {
            profile,
            overrides_applied: Vec::new(),
            resolutions: BTreeMap::new(),
        }
    }

    /// Resolve and record the tolerance for one component.
    pub fn resolve(
        &mut self,
        component: &str,
        unit: Option<&Unit>,
        source: Option<SourceTol>,
    ) -> Tol {
        if let Some(r) = self.resolutions.get(component) {
            return r.tol;
        }
        let resolved = self.profile.resolve(component, unit, source);
        if resolved.rule != Rule::Profile {
            self.overrides_applied.push(OverrideApplied {
                component: component.to_string(),
                rule: resolved.rule.clone(),
                tol: resolved.tol,
                looser: resolved.tol.is_looser_than(&self.profile.base),
                note: match resolved.rule {
                    Rule::SourcePrecision(_) => {
                        Some("tolerance_raised_by_source_precision".to_string())
                    }
                    _ => None,
                },
            });
            self.overrides_applied
                .sort_by(|x, y| x.component.cmp(&y.component));
        }
        self.resolutions
            .insert(component.to_string(), resolved.clone());
        resolved.tol
    }

    /// The overrides that permit more difference than the profile.
    pub fn loosenings(&self) -> Vec<&OverrideApplied> {
        self.overrides_applied.iter().filter(|o| o.looser).collect()
    }

    /// The `tolerance` object of `diff.json`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "profile": self.profile.name,
            "abs": self.profile.base.abs,
            "rel": self.profile.base.rel,
            "money_dp": self.profile.money_dp,
            "by_unit": self.profile.by_unit,
            "by_component": self.profile.by_component,
            "overrides_applied": self.overrides_applied.iter().map(OverrideApplied::to_json).collect::<Vec<_>>(),
            "loosened": self.loosenings().len(),
        })
    }
}

/// The IR unit's name as a per-unit override key.
pub fn unit_name(unit: &Unit) -> String {
    unit.to_string()
}

/// The observed decimal precision of a set of values (§5.6 rounding rule 4).
///
/// A Prophet `.rpt` is written at fixed precision, and comparing a full-precision engine against
/// it produces a phantom diff in every cell. The reader's own numbers are the evidence: the
/// maximum number of decimals any value carries is the precision the file was written at.
/// Capped at 12, above which the values are plainly not fixed-precision.
pub fn observed_precision(values: impl Iterator<Item = f64>) -> Option<u32> {
    let mut max = 0u32;
    let mut seen = 0usize;
    for v in values {
        if !v.is_finite() {
            continue;
        }
        seen += 1;
        // The *shortest* decimal representation, not the binary value: `{}` on f64 is the
        // shortest round-tripping form, which is exactly the "2.675 trap" guard §5.6 asks for.
        let text = format!("{v}");
        if text.contains('e') || text.contains('E') {
            return None;
        }
        let dp = text
            .split_once('.')
            .map(|(_, frac)| frac.len())
            .unwrap_or(0) as u32;
        if dp > 12 {
            return None;
        }
        max = max.max(dp);
    }
    (seen > 0).then_some(max)
}

/// `round(x, dp)`, round-half-away-from-zero on the decimal representation (§5.6 rounding rule 2).
///
/// Deliberately *not* IEEE round-half-even: Prophet and actuarial convention round half away from
/// zero, and the diff renders numbers the same way the models it compares do.
///
/// ```
/// use predictable_rundiff::tolerance::round_half_away;
/// assert_eq!(round_half_away(2.5, 0), 3.0);
/// assert_eq!(round_half_away(-2.5, 0), -3.0);
/// assert_eq!(round_half_away(2.675, 2), 2.68);   // the binary-representation trap
/// ```
pub fn round_half_away(x: f64, dp: u32) -> f64 {
    if !x.is_finite() {
        return x;
    }
    // Round through the *shortest* decimal representation rather than through `x * 10^dp` or
    // `{:.dp}`: both look at the binary value, where 2.675 is 2.67499…, and turn it into 2.67.
    let text = format!("{x}");
    if text.contains('e') || text.contains('E') {
        return x;
    }
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => (-1.0, rest.to_string()),
        None => (1.0, text),
    };
    let (int, frac) = digits.split_once('.').unwrap_or((digits.as_str(), ""));
    let dp = dp as usize;
    if frac.len() <= dp {
        return x;
    }
    let mut kept: Vec<u8> = format!("{int}{:0<width$}", &frac[..dp], width = dp)
        .bytes()
        .map(|b| b - b'0')
        .collect();
    // Half away from zero: the first dropped digit decides, and 5 always rounds up in magnitude.
    if frac.as_bytes()[dp] >= b'5' {
        let mut i = kept.len();
        loop {
            if i == 0 {
                kept.insert(0, 1);
                break;
            }
            i -= 1;
            if kept[i] == 9 {
                kept[i] = 0;
            } else {
                kept[i] += 1;
                break;
            }
        }
    }
    let all: String = kept.iter().map(|d| (d + b'0') as char).collect();
    let split = all.len() - dp;
    let rebuilt = if dp == 0 {
        all
    } else {
        format!("{}.{}", &all[..split], &all[split..])
    };
    sign * rebuilt.parse::<f64>().unwrap_or(x.abs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_predicate_is_symmetric() {
        let t = Tol::new(0.0, 1e-6);
        // With max(|a|,|b|) the pair matches from both directions; with |a| it would not.
        assert_eq!(matches(1.0, 1.0000009, t), matches(1.0000009, 1.0, t));
        assert!(matches(0.0, 0.0, Tol::new(0.0, 0.0)));
    }

    #[test]
    fn reconcile_tightens_probabilities_and_is_recorded_as_not_looser() {
        let mut r = ToleranceReport::new(Profile::builtin("reconcile").unwrap());
        let tol = r.resolve("model.qx", Some(&Unit::Prob), None);
        assert_eq!(tol, Tol::new(1e-9, 1e-9));
        assert_eq!(r.overrides_applied.len(), 1);
        assert!(!r.overrides_applied[0].looser);
        assert!(r.loosenings().is_empty());
    }

    #[test]
    fn a_per_component_loosening_is_flagged() {
        let profile = Profile::builtin("reconcile")
            .unwrap()
            .with_component("model.bel", Tol::new(5.0, 1e-3));
        let mut r = ToleranceReport::new(profile);
        assert_eq!(r.resolve("model.bel", Some(&Unit::Money), None).abs, 5.0);
        assert_eq!(r.loosenings().len(), 1);
        assert_eq!(r.loosenings()[0].rule, Rule::Component("model.bel".into()));
    }

    #[test]
    fn source_precision_raises_but_never_lowers() {
        let mut r = ToleranceReport::new(Profile::builtin("reconcile").unwrap());
        // 2dp source → 0.005, which is exactly `reconcile`'s money tolerance, so no raise.
        assert_eq!(
            r.resolve("a.x", Some(&Unit::Money), Some(SourceTol::new(2, 1.0)))
                .abs,
            0.005
        );
        // 1dp source → 0.05, a genuine raise, recorded with the required note.
        assert_eq!(
            r.resolve("a.y", Some(&Unit::Money), Some(SourceTol::new(1, 1.0)))
                .abs,
            0.05
        );
        let o = r
            .overrides_applied
            .iter()
            .find(|o| o.component == "a.y")
            .unwrap();
        assert!(o.looser);
        assert_eq!(
            o.note.as_deref(),
            Some("tolerance_raised_by_source_precision")
        );
    }

    #[test]
    fn observed_precision_reads_the_shortest_decimal_form() {
        assert_eq!(observed_precision([1.0, 2.25, 3.5].into_iter()), Some(2));
        assert_eq!(observed_precision([10.0f64, 20.0].into_iter()), Some(0));
        assert_eq!(observed_precision(std::iter::empty::<f64>()), None);
        assert_eq!(observed_precision([1.0f64 / 3.0].into_iter()), None);
    }

    #[test]
    fn exact_is_exact() {
        let p = Profile::builtin("exact").unwrap();
        assert_eq!(p.base, Tol::new(0.0, 0.0));
        assert!(!matches(1.0, 1.0 + f64::EPSILON, p.base));
        assert!(matches(1.0, 1.0, p.base));
    }

    #[test]
    fn an_unknown_profile_names_the_ones_that_exist() {
        let err = Profile::builtin("loose").unwrap_err();
        assert!(format!("{err}").contains("reconcile"));
    }
}
