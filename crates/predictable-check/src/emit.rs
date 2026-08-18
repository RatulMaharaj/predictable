//! Building `predictable-diagnostics` values from syntax spans.
//!
//! Every diagnostic the checker emits carries a primary span *and* at least one
//! suggested edit (`01-ir.md` §7). The helpers here make the edit the easy part:
//! [`replace`] and [`delete`] take a syntax [`Span`] and produce a literal
//! byte-range [`Edit`], in the same units the span uses.

use predictable_diagnostics::{
    registry, Applicability, Diagnostic, Edit, Severity, Span as DSpan, Suggestion,
};
use predictable_syntax::source::{SourceMap, Span};

/// Start a diagnostic for `code`.
///
/// Registered codes take their severity and `doc_url` from the registry. A code
/// the registry does not know — the parser's `E00xx` grammar codes, which are
/// owned by `predictable-syntax` — still renders, with the anchor it would have,
/// rather than panicking in the middle of a check.
pub fn diagnostic(code: &str, message: impl Into<String>) -> Diagnostic {
    let message = message.into();
    Diagnostic::try_new(code, message.clone()).unwrap_or_else(|| Diagnostic {
        code: code.to_string(),
        severity: if code.starts_with('W') {
            Severity::Warning
        } else {
            Severity::Error
        },
        message,
        spans: Vec::new(),
        suggestions: Vec::new(),
        doc_url: registry::doc_url_for(code),
        notes: Vec::new(),
    })
}

/// A primary span with a label.
pub fn primary(map: &SourceMap, span: Span, label: impl Into<String>) -> DSpan {
    DSpan::primary(map.name(span.file), range(span)).label(label)
}

/// A secondary span: "declared here", "but used here".
pub fn secondary(map: &SourceMap, span: Span, label: impl Into<String>) -> DSpan {
    DSpan::secondary(map.name(span.file), range(span)).label(label)
}

/// A suggestion that replaces the text under `span`.
pub fn replace(
    map: &SourceMap,
    span: Span,
    replacement: impl Into<String>,
    message: impl Into<String>,
) -> Suggestion {
    Suggestion::new(message).edit(Edit::replace(map.name(span.file), range(span), replacement))
}

/// A suggestion that deletes the text under `span`.
pub fn delete(map: &SourceMap, span: Span, message: impl Into<String>) -> Suggestion {
    Suggestion::new(message).edit(Edit::delete(map.name(span.file), range(span)))
}

/// A suggestion the author should read before applying — a rewrite that changes
/// what the model computes, not just how it is spelled.
pub fn maybe(suggestion: Suggestion) -> Suggestion {
    suggestion.applicability(Applicability::MaybeIncorrect)
}

/// Whole-line span of the line a span starts on, so a key can be deleted with
/// its newline.
pub fn line_range(map: &SourceMap, span: Span) -> std::ops::Range<usize> {
    let text = map.text(span.file);
    let start = text[..span.start as usize]
        .rfind('\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    let end = text[span.start as usize..]
        .find('\n')
        .map(|i| span.start as usize + i + 1)
        .unwrap_or(text.len());
    start..end
}

fn range(span: Span) -> std::ops::Range<usize> {
    span.start as usize..span.end as usize
}

/// Convert a parser diagnostic (pass 1) into the project-wide form.
pub fn from_syntax(map: &SourceMap, d: &predictable_syntax::Diagnostic) -> Diagnostic {
    let mut out = diagnostic(d.code, d.message.clone());
    out.severity = match d.severity {
        predictable_syntax::Severity::Error => Severity::Error,
        predictable_syntax::Severity::Warning => Severity::Warning,
        predictable_syntax::Severity::Note => Severity::Info,
        predictable_syntax::Severity::Help => Severity::Help,
    };
    for label in &d.labels {
        out.spans.push(if label.primary {
            primary(map, label.span, label.message.clone())
        } else {
            secondary(map, label.span, label.message.clone())
        });
    }
    for s in &d.suggestions {
        out.suggestions.push(replace(
            map,
            s.span,
            s.replacement.clone(),
            s.message.clone(),
        ));
    }
    out
}

/// The span of a quoted value's *contents*.
///
/// A raw TOML value's span includes its quotes; a diagnostic points at what the
/// author wrote, which is what is inside them — the column convention the
/// conformance corpus pins (`conformance/README.md` §3).
pub fn inner(span: Span) -> Span {
    if span.len() >= 2 {
        Span::new(span.file, span.start as usize + 1, span.end as usize - 1)
    } else {
        span
    }
}
