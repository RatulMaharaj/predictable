//! Parser-owned diagnostics.
//!
//! `T03` owns the project-wide diagnostics crate (rendering, `--json`, the code
//! registry). This crate must be usable before that exists, so it defines the
//! same *shape* — `{code, severity, message, spans, suggestions, doc_url}`,
//! `01-ir.md` §7 — with plain public fields, so a `From<syntax::Diagnostic>` in
//! the diagnostics crate is a field-by-field move and nothing here has to change.

use crate::source::Span;

/// Diagnostic severity. Matches the severities in `01-ir.md` §7.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Note,
    Help,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
            Severity::Help => "help",
        }
    }
}

/// A span with an explanatory message. Exactly one label per diagnostic is
/// `primary`; it is the caret line in the rendered form.
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub span: Span,
    pub message: String,
    pub primary: bool,
}

impl Label {
    pub fn primary(span: Span, message: impl Into<String>) -> Label {
        Label {
            span,
            message: message.into(),
            primary: true,
        }
    }

    pub fn secondary(span: Span, message: impl Into<String>) -> Label {
        Label {
            span,
            message: message.into(),
            primary: false,
        }
    }
}

/// A literal replacement edit over a byte range, mechanically appliable by an
/// agent (`01-ir.md` §7). `replacement` may be empty (a deletion).
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub span: Span,
    pub replacement: String,
    pub message: String,
}

impl Suggestion {
    pub fn new(
        span: Span,
        replacement: impl Into<String>,
        message: impl Into<String>,
    ) -> Suggestion {
        Suggestion {
            span,
            replacement: replacement.into(),
            message: message.into(),
        }
    }
}

/// A single parser diagnostic.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    /// Registry code, e.g. `E0011`. Syntax codes are `E00xx`; see the crate docs.
    pub code: &'static str,
    pub severity: Severity,
    pub message: String,
    pub labels: Vec<Label>,
    pub suggestions: Vec<Suggestion>,
    pub doc_url: String,
}

impl Diagnostic {
    pub fn error(code: &'static str, message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(code, Severity::Error, message)
    }

    pub fn warning(code: &'static str, message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(code, Severity::Warning, message)
    }

    pub fn new(code: &'static str, severity: Severity, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            code,
            severity,
            message: message.into(),
            labels: Vec::new(),
            suggestions: Vec::new(),
            doc_url: format!("https://predictable.dev/diagnostics/{code}"),
        }
    }

    pub fn with_primary(mut self, span: Span, message: impl Into<String>) -> Diagnostic {
        self.labels.push(Label::primary(span, message));
        self
    }

    pub fn with_secondary(mut self, span: Span, message: impl Into<String>) -> Diagnostic {
        self.labels.push(Label::secondary(span, message));
        self
    }

    pub fn with_suggestion(
        mut self,
        span: Span,
        replacement: impl Into<String>,
        message: impl Into<String>,
    ) -> Diagnostic {
        self.suggestions
            .push(Suggestion::new(span, replacement, message));
        self
    }

    /// The primary span, if the diagnostic has one.
    pub fn primary_span(&self) -> Option<Span> {
        self.labels.iter().find(|l| l.primary).map(|l| l.span)
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

/// Collector that also enforces the "yield multiple diagnostics" contract: the
/// parser keeps going after an error, and this is where the run's errors pile up.
#[derive(Debug, Default, Clone)]
pub struct Diagnostics {
    items: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn new() -> Diagnostics {
        Diagnostics { items: Vec::new() }
    }

    pub fn push(&mut self, d: Diagnostic) {
        self.items.push(d);
    }

    pub fn has_errors(&self) -> bool {
        self.items.iter().any(Diagnostic::is_error)
    }

    pub fn error_count(&self) -> usize {
        self.items.iter().filter(|d| d.is_error()).count()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Diagnostic> {
        self.items.iter()
    }

    pub fn into_vec(self) -> Vec<Diagnostic> {
        self.items
    }

    pub fn codes(&self) -> Vec<&'static str> {
        self.items.iter().map(|d| d.code).collect()
    }
}
