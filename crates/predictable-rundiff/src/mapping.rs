//! `migration/mapping.toml` — component-name correspondence as data (`04-verify.md` §4.4).
//!
//! The three adjustments a mapping entry may carry — `sign`, `scale`, `timing_shift` — are the
//! entirety of the "same number in different clothes" space. Each is a *declared, reviewable
//! claim*, so each is echoed into `diff.json` under `mapping.adjusted` and printed in the report
//! header. A fudge applied invisibly inside a comparison would be indistinguishable from a bug.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::DiffError;

/// `mp_key = { prophet = "POL_NUM", predictable = "policy_number" }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyMap {
    /// The Prophet-side column.
    pub prophet: String,
    /// The predictable-side field.
    pub predictable: String,
}

/// One `[[component]]` entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentMap {
    /// The Prophet column name, e.g. `PREM_INC`.
    pub prophet: String,
    /// The predictable component, qualified or not.
    pub predictable: String,
    /// `1 | -1`, for convention flips.
    #[serde(default = "one_i32")]
    pub sign: i32,
    /// e.g. `1000.0` where Prophet reports in thousands.
    #[serde(default = "one_f64")]
    pub scale: f64,
    /// Integer periods, for an in-advance/in-arrears mismatch.
    #[serde(default)]
    pub timing_shift: i32,
}

fn one_i32() -> i32 {
    1
}
fn one_f64() -> f64 {
    1.0
}

impl ComponentMap {
    /// True when this entry changes a number, and so must be printed.
    pub fn is_adjusted(&self) -> bool {
        self.sign != 1 || self.scale != 1.0 || self.timing_shift != 0
    }

    /// Apply the declared adjustment to a value.
    pub fn adjust(&self, v: f64) -> f64 {
        f64::from(self.sign) * self.scale * v
    }
}

/// One `[[unmapped]]` entry: a Prophet column with no counterpart, and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Unmapped {
    /// The Prophet column.
    pub prophet: String,
    /// The declared reason. Present so an unmapped column is a decision, not an oversight.
    #[serde(default)]
    pub reason: String,
}

/// A parsed `mapping.toml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Mapping {
    /// `pvf/1`.
    #[serde(default)]
    pub format: String,
    /// The Prophet run this mapping describes.
    #[serde(default)]
    pub prophet_run: Option<String>,
    /// The predictable module it maps onto.
    #[serde(default)]
    pub model_module: Option<String>,
    /// `0 | 1` — the Prophet period base (§4.3 rule 3).
    #[serde(default)]
    pub period_base: Option<i32>,
    /// The modelpoint key correspondence.
    #[serde(default)]
    pub mp_key: Option<KeyMap>,
    /// The component correspondences.
    #[serde(default, rename = "component")]
    pub components: Vec<ComponentMap>,
    /// Columns declared to have no counterpart.
    #[serde(default, rename = "unmapped")]
    pub unmapped: Vec<Unmapped>,

    /// Where it was read from. Not part of the file.
    #[serde(skip)]
    pub file: String,
    /// `sha256:…` of the bytes read. Not part of the file.
    #[serde(skip)]
    pub digest: String,
}

impl Mapping {
    /// Read and parse a mapping file.
    pub fn load(path: impl AsRef<Path>) -> Result<Mapping, DiffError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|e| DiffError::Mapping {
            path: path.display().to_string(),
            why: e.to_string(),
        })?;
        let text = String::from_utf8(bytes.clone()).map_err(|e| DiffError::Mapping {
            path: path.display().to_string(),
            why: e.to_string(),
        })?;
        let mut mapping: Mapping = toml::from_str(&text).map_err(|e| DiffError::Mapping {
            path: path.display().to_string(),
            why: e.message().to_string(),
        })?;
        mapping.file = path.display().to_string();
        mapping.digest = format!("sha256:{}", crate::sha256_hex(&bytes));
        Ok(mapping)
    }

    /// `prophet name → entry`.
    pub fn by_prophet(&self) -> BTreeMap<&str, &ComponentMap> {
        self.components
            .iter()
            .map(|c| (c.prophet.as_str(), c))
            .collect()
    }

    /// The entries that change a number.
    pub fn adjusted(&self) -> Vec<&ComponentMap> {
        self.components.iter().filter(|c| c.is_adjusted()).collect()
    }

    /// The Prophet columns declared unmapped.
    pub fn is_declared_unmapped(&self, prophet: &str) -> bool {
        // Case-insensitively, for the same reason component names are: the file says
        // `RESERVE_INT` and the importer wrote `prophet.reserve_int`.
        self.unmapped
            .iter()
            .any(|u| u.prophet.eq_ignore_ascii_case(prophet))
    }
}

/// Match a mapping entry's `predictable` name against a qualified component id.
///
/// `predictable = "renewal_expenses"` matches `model.renewal_expenses`, because a mapping file is
/// written by a human against the model's own vocabulary, not against module paths.
pub fn matches_predictable(entry: &ComponentMap, id: &str) -> bool {
    entry.predictable.eq_ignore_ascii_case(id)
        || id
            .rsplit_once('.')
            .map(|(_, tail)| tail.eq_ignore_ascii_case(&entry.predictable))
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
format = "pvf/1"
prophet_run = "TERM_BASE_2026Q2"
model_module = "term_assurance"
period_base = 1
mp_key = { prophet = "POL_NUM", predictable = "policy_number" }

[[component]]
prophet = "PREM_INC"
predictable = "premium_income"
sign = 1
scale = 1000.0

[[component]]
prophet = "BEL_TOT"
predictable = "bel"

[[unmapped]]
prophet = "RESERVE_INT"
reason = "intermediate; no predictable equivalent"
"#;

    #[test]
    fn defaults_are_the_identity_adjustment() {
        let m: Mapping = toml::from_str(SAMPLE).unwrap();
        let bel = &m.components[1];
        assert_eq!((bel.sign, bel.scale, bel.timing_shift), (1, 1.0, 0));
        assert!(!bel.is_adjusted());
    }

    #[test]
    fn only_the_entries_that_move_a_number_are_adjusted() {
        let m: Mapping = toml::from_str(SAMPLE).unwrap();
        let adjusted: Vec<&str> = m.adjusted().iter().map(|c| c.prophet.as_str()).collect();
        assert_eq!(adjusted, vec!["PREM_INC"]);
        assert_eq!(m.components[0].adjust(2.0), 2000.0);
    }

    #[test]
    fn a_qualified_id_matches_an_unqualified_mapping_entry() {
        let m: Mapping = toml::from_str(SAMPLE).unwrap();
        assert!(matches_predictable(&m.components[1], "model.bel"));
        assert!(matches_predictable(&m.components[1], "bel"));
        assert!(!matches_predictable(&m.components[1], "model.rebel"));
    }

    #[test]
    fn unmapped_is_a_declaration_with_a_reason() {
        let m: Mapping = toml::from_str(SAMPLE).unwrap();
        assert!(m.is_declared_unmapped("RESERVE_INT"));
        assert!(!m.is_declared_unmapped("PREM_INC"));
        assert_eq!(m.mp_key.unwrap().predictable, "policy_number");
        assert_eq!(m.period_base, Some(1));
    }
}
