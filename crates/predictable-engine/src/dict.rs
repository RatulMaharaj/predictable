//! The string dictionary.
//!
//! `str`, `date`-as-text and `enum` values are dictionary-encoded to a code the
//! kernel carries in an ordinary `f64` lane (`03-engine.md` §4.2). Only equality
//! is defined on them (`01-ir.md` §2.6), so comparing codes is comparing values,
//! and enums never compare by ordinal — the code is an interning artefact, not
//! an order.
//!
//! Codes are assigned in **first-intern order**, and the caller interns the
//! tape's literals before any modelpoint data, so the same model produces the
//! same codes on every machine. Nothing observable depends on a code's value.

use std::collections::BTreeMap;

/// An interning table shared by a whole run.
#[derive(Debug, Clone, Default)]
pub struct Dictionary {
    values: Vec<String>,
    index: BTreeMap<String, u32>,
}

impl Dictionary {
    pub fn new() -> Dictionary {
        Dictionary::default()
    }

    /// Intern `s`, returning its lane code.
    pub fn intern(&mut self, s: &str) -> f64 {
        if let Some(&code) = self.index.get(s) {
            return f64::from(code);
        }
        let code = self.values.len() as u32;
        self.values.push(s.to_string());
        self.index.insert(s.to_string(), code);
        f64::from(code)
    }

    /// The code of an already-interned string; `-1.0` for an unknown value,
    /// which compares equal to nothing and therefore misses every lookup.
    pub fn code_of(&self, s: &str) -> f64 {
        self.index.get(s).map(|&c| f64::from(c)).unwrap_or(-1.0)
    }

    /// The string behind a lane code, or `""` when the code is the unknown
    /// marker.
    pub fn decode(&self, code: f64) -> &str {
        let i = code as i64;
        if i < 0 {
            return "";
        }
        self.values
            .get(i as usize)
            .map(String::as_str)
            .unwrap_or("")
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}
