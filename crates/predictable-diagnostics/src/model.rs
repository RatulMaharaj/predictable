//! The diagnostic value model: `{code, severity, message, spans, suggestions, doc_url}`.
//!
//! Normative shape: `01-ir.md` §7, `02-dsl.md` §10, `04-verify.md` §8.3.

use serde::{Deserialize, Serialize};
use std::ops::Range;

use crate::registry::{self, doc_url_for};

/// How loud a diagnostic is.
///
/// `Error` and `Warning` are the checker's two levels; `Info` covers trace notes
/// (`N0xxx`), reader observations and diff hypotheses (`H0xxx`); `Help` is only
/// ever attached to a suggestion, never to a top-level diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Blocks the run. Exit code 2 territory.
    Error,
    /// A lint. The run proceeds.
    Warning,
    /// Informational: notes, hypotheses, reader observations.
    Info,
    /// Attached to suggestions.
    Help,
}

impl Severity {
    /// The word used in rendered terminal output and in `--json`.
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
            Severity::Help => "help",
        }
    }

    /// True if this severity should fail a `predictable check`.
    pub fn is_fatal(self) -> bool {
        matches!(self, Severity::Error)
    }
}

/// A 1-based line/column pair, resolved from a byte offset on demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineCol {
    /// 1-based line number.
    pub line: usize,
    /// 1-based column, counted in characters (not bytes).
    pub column: usize,
}

/// A byte range in a named source file, with an optional label.
///
/// Offsets are **byte** offsets into the file's UTF-8 bytes — the same units the
/// suggested edits use, so an agent can apply an edit without re-deriving
/// positions from line/column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    /// Path of the file the span points into, as written by the caller.
    pub file: String,
    /// Inclusive start byte offset.
    pub start: usize,
    /// Exclusive end byte offset.
    pub end: usize,
    /// Whether this is the primary span (the caret) or a secondary one.
    pub primary: bool,
    /// Short label rendered under the span.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Filled in by [`crate::Diagnostic::resolve_positions`]; absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_pos: Option<LineCol>,
}

impl Span {
    /// A primary span (the one the caret points at).
    pub fn primary(file: impl Into<String>, range: Range<usize>) -> Self {
        Span {
            file: file.into(),
            start: range.start,
            end: range.end,
            primary: true,
            label: None,
            start_pos: None,
        }
    }

    /// A secondary span: context, "declared here", "but used here".
    pub fn secondary(file: impl Into<String>, range: Range<usize>) -> Self {
        Span {
            primary: false,
            ..Span::primary(file, range)
        }
    }

    /// Attach a short label rendered beneath the underline.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The span as a `Range<usize>`.
    pub fn range(&self) -> Range<usize> {
        self.start..self.end
    }
}

/// A single literal replacement: replace `start..end` in `file` with `replacement`.
///
/// Deleting is `replacement = ""`; inserting is `start == end`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edit {
    /// File the edit applies to.
    pub file: String,
    /// Inclusive start byte offset.
    pub start: usize,
    /// Exclusive end byte offset.
    pub end: usize,
    /// Literal text to put in place of `start..end`.
    pub replacement: String,
}

impl Edit {
    /// Replace `range` in `file` with `replacement`.
    pub fn replace(
        file: impl Into<String>,
        range: Range<usize>,
        replacement: impl Into<String>,
    ) -> Self {
        Edit {
            file: file.into(),
            start: range.start,
            end: range.end,
            replacement: replacement.into(),
        }
    }

    /// Insert `text` at byte offset `at`.
    pub fn insert(file: impl Into<String>, at: usize, text: impl Into<String>) -> Self {
        Edit::replace(file, at..at, text)
    }

    /// Delete `range`.
    pub fn delete(file: impl Into<String>, range: Range<usize>) -> Self {
        Edit::replace(file, range, "")
    }

    /// The edit's target range.
    pub fn range(&self) -> Range<usize> {
        self.start..self.end
    }
}

/// How confident the emitter is that applying a suggestion is correct.
///
/// An agent in the migration loop applies `MachineApplicable` edits without
/// asking; anything else it shows to a human.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Applicability {
    /// Safe to apply mechanically; the result is what the emitter meant.
    MachineApplicable,
    /// Probably right, but a human should look.
    MaybeIncorrect,
    /// Contains placeholders the caller must fill in.
    HasPlaceholders,
    /// Illustrative only; do not apply.
    Unspecified,
}

/// A named fix: a message plus the literal byte-range edits that implement it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suggestion {
    /// What applying this does, phrased as an instruction ("use `when(...)` instead").
    pub message: String,
    /// The edits, in no particular order; [`crate::apply_edits`] sorts them.
    pub edits: Vec<Edit>,
    /// Confidence in the fix.
    pub applicability: Applicability,
}

impl Suggestion {
    /// A machine-applicable suggestion with no edits yet.
    pub fn new(message: impl Into<String>) -> Self {
        Suggestion {
            message: message.into(),
            edits: Vec::new(),
            applicability: Applicability::MachineApplicable,
        }
    }

    /// Add an edit.
    pub fn edit(mut self, edit: Edit) -> Self {
        self.edits.push(edit);
        self
    }

    /// Override the applicability (default [`Applicability::MachineApplicable`]).
    pub fn applicability(mut self, applicability: Applicability) -> Self {
        self.applicability = applicability;
        self
    }
}

/// One diagnostic, as rendered on the terminal and as serialized under `--json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Registry code, e.g. `E0201`.
    pub code: String,
    /// Severity, defaulted from the registry.
    pub severity: Severity,
    /// One line, lower case, no trailing period — the title line.
    pub message: String,
    /// Primary span first by convention; [`Diagnostic::primary_span`] does not rely on order.
    pub spans: Vec<Span>,
    /// Zero or more named fixes.
    pub suggestions: Vec<Suggestion>,
    /// Anchor into the diagnostics catalogue for this code.
    pub doc_url: String,
    /// Free prose paragraphs rendered after the snippet: why the rule exists.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl Diagnostic {
    /// Start a diagnostic for a registered code, taking severity and `doc_url`
    /// from the registry.
    ///
    /// # Panics
    /// If `code` is not in the registry. Use [`Diagnostic::try_new`] to handle
    /// that case; an unregistered code is a bug, because it would ship with no
    /// documentation anchor.
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Diagnostic::try_new(code, message)
            .unwrap_or_else(|| panic!("diagnostic code `{code}` is not in the registry"))
    }

    /// Like [`Diagnostic::new`], but returns `None` for an unregistered code.
    pub fn try_new(code: &str, message: impl Into<String>) -> Option<Self> {
        let info = registry::lookup(code)?;
        Some(Diagnostic {
            code: info.code.to_string(),
            severity: info.severity,
            message: message.into(),
            spans: Vec::new(),
            suggestions: Vec::new(),
            doc_url: doc_url_for(info.code),
            notes: Vec::new(),
        })
    }

    /// Override the registry's default severity (e.g. a lint promoted by `--deny`).
    pub fn severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    /// Add a span.
    pub fn span(mut self, span: Span) -> Self {
        self.spans.push(span);
        self
    }

    /// Add a suggestion.
    pub fn suggestion(mut self, suggestion: Suggestion) -> Self {
        self.suggestions.push(suggestion);
        self
    }

    /// Add an explanatory paragraph.
    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// The first primary span, if any.
    pub fn primary_span(&self) -> Option<&Span> {
        self.spans.iter().find(|s| s.primary)
    }

    /// The registry entry for this diagnostic's code.
    pub fn info(&self) -> Option<&'static registry::CodeInfo> {
        registry::lookup(&self.code)
    }

    /// Every file this diagnostic touches, in first-seen order (spans then edits).
    pub fn files(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        let spans = self.spans.iter().map(|s| s.file.as_str());
        let edits = self
            .suggestions
            .iter()
            .flat_map(|s| s.edits.iter().map(|e| e.file.as_str()));
        for f in spans.chain(edits) {
            if !out.contains(&f) {
                out.push(f);
            }
        }
        out
    }

    /// Fill `Span::start_pos` for every span whose file is present in `sources`.
    ///
    /// The JSON form carries byte offsets as the normative positions; line/column
    /// is a convenience for humans and is only present after this call.
    pub fn resolve_positions(&mut self, sources: &crate::SourceMap) {
        for span in &mut self.spans {
            if let Some(src) = sources.get(&span.file) {
                span.start_pos = Some(crate::line_col(src, span.start));
            }
        }
    }
}
