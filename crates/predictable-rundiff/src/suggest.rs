//! Suggested edits: a hypothesis that cannot be applied is only half a hypothesis.
//!
//! `04-verify.md` §5.5: *"`suggested_edit` uses the same `{file, byte_start, byte_end, old, new}`
//! shape as IR §7 suggestions, so one code path in the agent applies edits from both the checker
//! and the differ."*
//!
//! Two rules make an edit trustworthy enough for an agent to apply without asking:
//!
//! 1. **`old` is the bytes that are there.** Every [`SuggestedEdit`] is built by reading
//!    `text[byte_start..byte_end]` out of the file on disk, never by reconstructing what the
//!    detector thinks is there. [`SuggestedEdit::still_applies`] re-checks it against the current
//!    bytes, so an edit proposed against a file that has since been edited fails loudly.
//! 2. **The anchor comes from the parser, not from a search.** Spans are taken from
//!    [`predictable_syntax::parse_raw`], so `expr = "a * b"` is anchored at its value token even
//!    when the same text appears in a `doc` string three components later.

use std::path::{Path, PathBuf};

use predictable_syntax::raw::{Section, Value};
use predictable_syntax::{parse_raw, Diagnostics, SourceMap};
use serde::{Deserialize, Serialize};

/// One literal replacement, in the shape IR §7 gives checker suggestions.
///
/// `new` replaces `old` at `[byte_start, byte_end)` of `file`. Ranges are byte offsets into the
/// file as it was read, because a character offset is a different number in every encoding and a
/// line/column pair is a different number after every `fmt`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestedEdit {
    /// The file, as the model manifest spelled its path.
    pub file: String,
    /// Inclusive start byte offset.
    pub byte_start: usize,
    /// Exclusive end byte offset.
    pub byte_end: usize,
    /// The bytes currently there — always read from the file, never assumed.
    pub old: String,
    /// The bytes to put in their place.
    pub new: String,
    /// What applying this does, phrased as an instruction.
    pub message: String,
}

impl SuggestedEdit {
    /// The same edit in the diagnostics crate's shape, so `apply_edits` takes it unchanged.
    pub fn to_edit(&self) -> predictable_diagnostics::Edit {
        predictable_diagnostics::Edit::replace(
            self.file.clone(),
            self.byte_start..self.byte_end,
            self.new.clone(),
        )
    }

    /// True when `text` still has `old` at the recorded range — the precondition for applying.
    pub fn still_applies(&self, text: &str) -> bool {
        text.get(self.byte_start..self.byte_end) == Some(self.old.as_str())
    }

    /// `text` with the edit applied, when it still applies.
    pub fn apply(&self, text: &str) -> Option<String> {
        if !self.still_applies(text) {
            return None;
        }
        let mut out = String::with_capacity(text.len() + self.new.len());
        out.push_str(&text[..self.byte_start]);
        out.push_str(&self.new);
        out.push_str(&text[self.byte_end..]);
        Some(out)
    }
}

/// A span located in one indexed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    /// The file path as written.
    pub file: String,
    /// Inclusive start byte offset.
    pub start: usize,
    /// Exclusive end byte offset.
    pub end: usize,
    /// The bytes in the range.
    pub text: String,
}

impl Anchor {
    /// An edit replacing the whole anchored range.
    pub fn replace(&self, new: impl Into<String>, message: impl Into<String>) -> SuggestedEdit {
        SuggestedEdit {
            file: self.file.clone(),
            byte_start: self.start,
            byte_end: self.end,
            old: self.text.clone(),
            new: new.into(),
            message: message.into(),
        }
    }

    /// An edit replacing the *contents* of a quoted string, re-quoting the way it was quoted.
    ///
    /// Returns `None` when the anchor is not a simple double-quoted string, or when `inner` would
    /// need escaping: a suggestion that needs an escaping pass is a suggestion this module is not
    /// entitled to make.
    pub fn replace_quoted(&self, inner: &str, message: impl Into<String>) -> Option<SuggestedEdit> {
        let quoted = self.text.strip_prefix('"')?.strip_suffix('"')?;
        if quoted.contains('\\') || inner.contains('"') || inner.contains('\\') {
            return None;
        }
        Some(self.replace(format!("\"{inner}\""), message))
    }

    /// The unquoted contents of the anchored range, when it is a simple double-quoted string.
    pub fn unquoted(&self) -> Option<&str> {
        self.text
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .filter(|s| !s.contains('\\'))
    }
}

/// One `.pir` file, parsed only far enough to know where every value token starts and ends.
#[derive(Debug, Clone)]
struct IndexedFile {
    path: String,
    text: String,
    sections: Vec<Section>,
}

/// The model's source files, indexed by span.
///
/// Built from the same paths the run manifest recorded and the run diff already verified the
/// digests of (`run_side::model_files`), so an edit this index proposes is an edit against *the
/// model that ran* — not against whatever is in the working tree now.
#[derive(Debug, Clone, Default)]
pub struct SourceIndex {
    files: Vec<IndexedFile>,
}

impl SourceIndex {
    /// An index over nothing. Every lookup returns `None`, so a diff without model sources emits
    /// hypotheses without `suggested_edit` rather than emitting no hypotheses.
    pub fn empty() -> SourceIndex {
        SourceIndex::default()
    }

    /// True when no file was indexed.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Index the given `.pir` files. Unreadable or unparsable files are skipped silently — a
    /// missing anchor costs a `suggested_edit`, and never a hypothesis.
    pub fn load(paths: &[PathBuf]) -> SourceIndex {
        let mut index = SourceIndex::default();
        for path in paths {
            let Ok(text) = std::fs::read_to_string(path) else {
                continue;
            };
            index.add(&display(path), &text);
        }
        index
    }

    /// Index one file from memory. Used by the tests, and by any caller holding the bytes already.
    pub fn add(&mut self, path: &str, text: &str) {
        let mut sources = SourceMap::new();
        let file = sources.add(path.to_string(), text.to_string());
        let mut diags = Diagnostics::new();
        let doc = parse_raw(text, file, &mut diags);
        self.files.push(IndexedFile {
            path: path.to_string(),
            text: text.to_string(),
            sections: doc.sections,
        });
    }

    /// The value span of `field` in the `[[component]]` block declaring `name`.
    pub fn component_field(&self, name: &str, field: &str) -> Option<Anchor> {
        self.field_in("component", name, field)
    }

    /// The value span of `field` in the `[[table]]` block declaring `name`.
    pub fn table_field(&self, name: &str, field: &str) -> Option<Anchor> {
        self.field_in("table", name, field)
    }

    /// The value span of `field` in the `[[assumption]]` block declaring `name`.
    pub fn assumption_field(&self, name: &str, field: &str) -> Option<Anchor> {
        self.field_in("assumption", name, field)
    }

    /// The `policy = "..."` span of one key column of one table.
    ///
    /// `keys` is an array of inline tables, so the anchor is inside a value inside a value; the
    /// raw parser keeps a span on each, which is exactly why the index is built from it rather
    /// than from a regex over the line.
    pub fn table_key_policy(&self, table: &str, key: &str) -> Option<Anchor> {
        for file in &self.files {
            let Some(section) = named(&file.sections, "table", table) else {
                continue;
            };
            let Some(keys) = section.table.get("keys") else {
                continue;
            };
            let Value::Array(items) = &keys.value else {
                continue;
            };
            for item in items {
                let Value::Table(inline) = &item.value else {
                    continue;
                };
                let is_key = matches!(inline.get("name").map(|v| &v.value),
                    Some(Value::Str(n)) if n == key);
                if !is_key {
                    continue;
                }
                if let Some(policy) = inline.get("policy") {
                    return file.anchor(policy.span.start as usize, policy.span.end as usize);
                }
            }
        }
        None
    }

    /// The single key column of a one-key table, when the table has exactly one.
    pub fn sole_key_name(&self, table: &str) -> Option<String> {
        for file in &self.files {
            let Some(section) = named(&file.sections, "table", table) else {
                continue;
            };
            let Some(Value::Array(items)) = section.table.get("keys").map(|v| &v.value) else {
                continue;
            };
            if items.len() != 1 {
                return None;
            }
            if let Value::Table(inline) = &items[0].value {
                if let Some(Value::Str(n)) = inline.get("name").map(|v| &v.value) {
                    return Some(n.clone());
                }
            }
        }
        None
    }

    fn field_in(&self, section_name: &str, name: &str, field: &str) -> Option<Anchor> {
        for file in &self.files {
            let Some(section) = named(&file.sections, section_name, name) else {
                continue;
            };
            if let Some(value) = section.table.get(field) {
                return file.anchor(value.span.start as usize, value.span.end as usize);
            }
        }
        None
    }
}

impl IndexedFile {
    fn anchor(&self, start: usize, end: usize) -> Option<Anchor> {
        let text = self.text.get(start..end)?.to_string();
        Some(Anchor {
            file: self.path.clone(),
            start,
            end,
            text,
        })
    }
}

fn named<'s>(sections: &'s [Section], header: &str, name: &str) -> Option<&'s Section> {
    sections.iter().find(|s| {
        s.is(header)
            && matches!(s.table.get("name").map(|v| &v.value), Some(Value::Str(n)) if n == name)
    })
}

fn display(p: &Path) -> String {
    p.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"format = "pir/1"
module = "m"

[[component]]
name = "premium_income"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "premium_rate * num_pols_if"
doc = "premium_rate * num_pols_if"

[[table]]
name = "lapses"
keys = [{ name = "policy_year", dtype = "i64", policy = "step" }]
values = [{ name = "lapse_pa", dtype = "f64", unit = "prob" }]
on_missing = "error"
source = "tables/lapses.csv"
"#;

    fn index() -> SourceIndex {
        let mut i = SourceIndex::empty();
        i.add("model.pir", SRC);
        i
    }

    #[test]
    fn a_component_field_anchors_at_its_own_value_not_the_first_matching_text() {
        let a = index().component_field("premium_income", "expr").unwrap();
        assert_eq!(a.text, "\"premium_rate * num_pols_if\"");
        // The identical text in `doc` is *after* the `expr`, and must not be what we anchored.
        assert_eq!(&SRC[a.start..a.end], a.text);
        assert!(SRC[..a.start].ends_with("expr = "));
    }

    #[test]
    fn the_edit_carries_the_bytes_that_are_there_and_re_quotes_the_replacement() {
        let a = index().component_field("premium_income", "expr").unwrap();
        let edit = a
            .replace_quoted(
                "premium_rate * num_pols_if * 12",
                "restore the annualisation",
            )
            .unwrap();
        assert!(edit.still_applies(SRC));
        let after = edit.apply(SRC).unwrap();
        assert!(after.contains("expr = \"premium_rate * num_pols_if * 12\""));
        // `doc` is untouched: the anchor was a span, not a search-and-replace.
        assert!(after.contains("doc = \"premium_rate * num_pols_if\""));
    }

    #[test]
    fn an_edit_against_changed_bytes_does_not_apply() {
        let a = index().component_field("premium_income", "expr").unwrap();
        let edit = a.replace_quoted("x", "…").unwrap();
        let moved = SRC.replace("[[component]]", "\n[[component]]");
        assert!(!edit.still_applies(&moved));
        assert!(edit.apply(&moved).is_none());
    }

    #[test]
    fn a_table_key_policy_anchors_inside_the_inline_table() {
        let a = index().table_key_policy("lapses", "policy_year").unwrap();
        assert_eq!(a.text, "\"step\"");
        let edit = a.replace("\"clamp\"", "hold the last rate beyond the table");
        assert!(edit.apply(SRC).unwrap().contains("policy = \"clamp\""));
        assert_eq!(
            index().sole_key_name("lapses").as_deref(),
            Some("policy_year")
        );
    }

    #[test]
    fn the_diagnostics_edit_shape_round_trips() {
        let a = index().component_field("premium_income", "timing").unwrap();
        let edit = a.replace("\"end\"", "move the cashflow to the year end");
        let d = edit.to_edit();
        assert_eq!(d.file, "model.pir");
        assert_eq!(d.range(), edit.byte_start..edit.byte_end);
        assert_eq!(d.replacement, "\"end\"");
    }

    #[test]
    fn an_empty_index_answers_none_rather_than_guessing() {
        let i = SourceIndex::empty();
        assert!(i.is_empty());
        assert!(i.component_field("premium_income", "expr").is_none());
    }
}
