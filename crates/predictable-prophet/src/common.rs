//! Pieces every reader shares: the read outcome, header scanning, and cells.

use std::collections::BTreeMap;
use std::ops::Range;

use predictable_diagnostics::{Diagnostic, Severity};

use crate::text::{split_fields, Field, Line};

/// The result of a reader: a value that always exists, plus the diagnostics that
/// describe how much of it can be trusted.
///
/// Readers never return `Err` and never panic. A file that cannot be interpreted
/// at all yields an empty value and at least one `Severity::Error` diagnostic —
/// which is what makes `bytes -> (Value, [Diagnostic])` a total function, and
/// what the fuzz tests assert.
#[derive(Debug, Clone, PartialEq)]
pub struct Read<T> {
    /// The parsed value, possibly partial.
    pub value: T,
    /// Every diagnostic raised, in emission order.
    pub diagnostics: Vec<Diagnostic>,
}

impl<T> Read<T> {
    /// Wrap a value with no diagnostics.
    pub fn new(value: T) -> Read<T> {
        Read {
            value,
            diagnostics: Vec::new(),
        }
    }

    /// True when any diagnostic is an error, i.e. the value must not be used as
    /// migration input without a human deciding otherwise.
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    /// The codes raised, in order — the handle tests and the CLI report on.
    pub fn codes(&self) -> Vec<&str> {
        self.diagnostics.iter().map(|d| d.code.as_str()).collect()
    }
}

/// One `KEY, value…` header line.
#[derive(Debug, Clone, PartialEq)]
pub struct HeaderEntry {
    /// The key, upper-cased.
    pub key: String,
    /// The payload fields, i.e. everything after the key.
    pub payload: Vec<Field>,
    /// Byte range of the whole line.
    pub span: Range<usize>,
    /// 1-based physical line number.
    pub line: usize,
}

impl HeaderEntry {
    /// The first payload field's text, or `""`.
    pub fn value(&self) -> &str {
        self.payload.first().map(|f| f.text.as_str()).unwrap_or("")
    }
}

/// The header block of an `.MPF` or `.rpt`: comments, recognised keys, and the
/// unrecognised keys retained verbatim (§4.1.2–4.1.3).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Header {
    /// `!` and `#` lines, in file order, decoded and with the marker kept.
    pub comments: Vec<String>,
    /// Recognised keys, by upper-cased key. Last occurrence wins.
    pub entries: BTreeMap<String, HeaderEntry>,
    /// Unrecognised `key, value` lines, retained verbatim (`P0102`).
    pub extra: BTreeMap<String, String>,
}

impl Header {
    /// The entry for a recognised key.
    pub fn get(&self, key: &str) -> Option<&HeaderEntry> {
        self.entries.get(key)
    }

    /// The first payload value of a recognised key.
    pub fn value(&self, key: &str) -> Option<&str> {
        self.get(key).map(|e| e.value())
    }
}

/// True for a comment or directive line (`!` or `#`), §4.1.2.
pub fn is_comment(line: &Line<'_>) -> bool {
    matches!(line.bytes.first(), Some(b'!') | Some(b'#'))
}

/// Split a header line into its key and payload.
pub fn header_entry(line: &Line<'_>) -> HeaderEntry {
    let fields = split_fields(line);
    let (key, payload) = match fields.split_first() {
        Some((k, rest)) => (k.key(), rest.to_vec()),
        None => (String::new(), Vec::new()),
    };
    HeaderEntry {
        key,
        payload,
        span: line.range(),
        line: line.number,
    }
}

/// A parsed cell of a Prophet data row.
///
/// `Null` is only ever produced by an empty field: a missing value in a
/// `required` field is an error at *load* time, not import time (§4.1.7).
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    /// An empty field.
    Null,
    /// `I`.
    Int(i64),
    /// `N`.
    Float(f64),
    /// `B`.
    Bool(bool),
    /// `S` / `T`.
    Str(String),
    /// `D`, normalised to ISO-8601 `YYYY-MM-DD`.
    Date(String),
}

impl Cell {
    /// CSV rendering: `Null` is the empty field, everything else its text form.
    pub fn to_csv_field(&self) -> String {
        match self {
            Cell::Null => String::new(),
            Cell::Int(v) => v.to_string(),
            Cell::Float(v) => crate::format_f64(*v),
            Cell::Bool(v) => v.to_string(),
            Cell::Str(v) => {
                if v.contains(',') || v.contains('"') || v.contains('\n') {
                    format!("\"{}\"", v.replace('"', "\"\""))
                } else {
                    v.clone()
                }
            }
            Cell::Date(v) => v.clone(),
        }
    }
}
