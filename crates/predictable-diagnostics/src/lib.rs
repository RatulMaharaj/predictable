//! Diagnostics for predictable.
//!
//! Every error, lint, note and hypothesis the toolchain produces is a
//! [`Diagnostic`]: `{code, severity, message, spans, suggestions, doc_url}`
//! (`01-ir.md` §7, `02-dsl.md` §10, `04-verify.md` §8.3). One value, two
//! renderings — `annotate-snippets` text for a terminal and JSON for `--json`,
//! which is what an LLM agent consumes in the migration loop.
//!
//! Three invariants hold the design together:
//!
//! 1. **Codes are registered.** [`Diagnostic::new`] refuses a code that is not in
//!    [`registry::REGISTRY`], and a test asserts every registered code has an
//!    anchor in `docs/llm/diagnostics.md`. A code therefore always has a `doc_url`
//!    that resolves.
//! 2. **Positions are byte offsets.** Spans and edits use the same units, so a
//!    suggested edit can be applied with [`apply_edits`] without re-deriving a
//!    position from line and column.
//! 3. **Suggestions are literal.** A suggestion is replacement text plus a range,
//!    never prose describing a fix, so applying one is mechanical.
//!
//! ```
//! use predictable_diagnostics::{apply_edits, Diagnostic, Edit, SourceMap, Span, Suggestion};
//!
//! let src = "expr = \"num_pols_iff * qx\"\n";
//! let mut sources = SourceMap::new();
//! sources.insert("decrements.pir", src);
//!
//! let diag = Diagnostic::new("E1103", "unknown name `num_pols_iff`")
//!     .span(Span::primary("decrements.pir", 8..20).label("not a component"))
//!     .suggestion(
//!         Suggestion::new("there is a component named `num_pols_if`")
//!             .edit(Edit::replace("decrements.pir", 8..20, "num_pols_if")),
//!     );
//!
//! assert_eq!(diag.doc_url, "https://predictable.dev/llm/diagnostics/#E1103");
//! let fixed = apply_edits(src, &diag.suggestions[0].edits).unwrap();
//! assert_eq!(fixed, "expr = \"num_pols_if * qx\"\n");
//! ```

#![deny(missing_docs)]

pub mod edits;
pub mod model;
pub mod registry;
pub mod render;

use std::collections::BTreeMap;

pub use edits::{apply_edits, apply_edits_to_file, EditError};
pub use model::{Applicability, Diagnostic, Edit, LineCol, Severity, Span, Suggestion};
pub use registry::{CodeInfo, Namespace, DOC_BASE, DOC_PATH, REGISTRY};
pub use render::{render, render_all, RenderOptions};

/// The source text rendering needs, keyed by the same file strings the spans use.
///
/// It is a plain map rather than a file-system reader on purpose: the checker
/// already holds every `.pir` file it parsed, and rendering must work in WASM and
/// in tests where there is no file system.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceMap {
    files: BTreeMap<String, String>,
}

impl SourceMap {
    /// An empty map.
    pub fn new() -> Self {
        SourceMap::default()
    }

    /// Register (or replace) a file's source text.
    pub fn insert(&mut self, file: impl Into<String>, source: impl Into<String>) -> &mut Self {
        self.files.insert(file.into(), source.into());
        self
    }

    /// The source text for `file`, if known.
    pub fn get(&self, file: &str) -> Option<&String> {
        self.files.get(file)
    }

    /// Whether `file` is known.
    pub fn contains(&self, file: &str) -> bool {
        self.files.contains_key(file)
    }

    /// The registered file paths, in sorted order.
    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for SourceMap {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut map = SourceMap::new();
        for (k, v) in iter {
            map.insert(k, v);
        }
        map
    }
}

/// Convert a byte offset into a 1-based line and character column.
///
/// An offset past the end of `source` clamps to the last position rather than
/// panicking: a diagnostic is never worth crashing a run for.
pub fn line_col(source: &str, offset: usize) -> LineCol {
    let offset = offset.min(source.len());
    let mut line = 1usize;
    let mut line_start = 0usize;
    for (i, b) in source.as_bytes()[..offset].iter().enumerate() {
        if *b == b'\n' {
            line += 1;
            line_start = i + 1;
        }
    }
    let column = source[line_start..offset].chars().count() + 1;
    LineCol { line, column }
}

/// Serialize a batch of diagnostics as the `--json` array.
///
/// The output is a JSON array of diagnostic objects — not an object with a
/// wrapper key — so a consumer can stream it and so an empty run is `[]`.
pub fn to_json(diagnostics: &[Diagnostic]) -> String {
    serde_json::to_string(diagnostics).expect("diagnostics are always serializable")
}

/// Pretty form of [`to_json`], for `--json` written to a file.
pub fn to_json_pretty(diagnostics: &[Diagnostic]) -> String {
    serde_json::to_string_pretty(diagnostics).expect("diagnostics are always serializable")
}

/// Parse a `--json` array back into diagnostics.
pub fn from_json(json: &str) -> Result<Vec<Diagnostic>, serde_json::Error> {
    serde_json::from_str(json)
}

/// The process exit code implied by a batch: 0 clean, 1 warnings only, 2 errors.
///
/// This is the CLI contract in `04-verify.md` §7; it lives here so every entry
/// point agrees on it.
pub fn exit_code(diagnostics: &[Diagnostic]) -> i32 {
    if diagnostics.iter().any(|d| d.severity == Severity::Error) {
        2
    } else if diagnostics.iter().any(|d| d.severity == Severity::Warning) {
        1
    } else {
        0
    }
}
