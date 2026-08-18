//! Terminal rendering, via `annotate-snippets`.
//!
//! Rendering is a pure function of `(diagnostic, sources, options)` — no
//! ambient terminal state, no environment lookups — so golden tests over the
//! rendered text are stable.

use annotate_snippets::{
    renderer::DecorStyle, AnnotationKind, Group, Level, Patch, Renderer, Snippet,
};

use crate::model::{Diagnostic, Severity};
use crate::SourceMap;

/// Rendering options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderOptions {
    /// Emit ANSI colour escapes. Off by default so output is diffable.
    pub color: bool,
    /// Use box-drawing characters instead of ASCII.
    pub unicode: bool,
    /// Wrap width.
    pub term_width: usize,
    /// Render the suggested edits as diff blocks after the snippet.
    pub show_suggestions: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions {
            color: false,
            unicode: false,
            term_width: 100,
            show_suggestions: true,
        }
    }
}

impl RenderOptions {
    /// Colourised output for an interactive terminal.
    pub fn colored() -> Self {
        RenderOptions {
            color: true,
            unicode: true,
            ..RenderOptions::default()
        }
    }
}

fn level(severity: Severity) -> Level<'static> {
    match severity {
        Severity::Error => Level::ERROR,
        Severity::Warning => Level::WARNING,
        Severity::Info => Level::INFO,
        Severity::Help => Level::HELP,
    }
}

/// Render one diagnostic as terminal text.
///
/// Spans whose file is absent from `sources` are dropped from the snippet and
/// reported as a bare `path:line` origin instead, so a diagnostic produced
/// against source the caller no longer holds still renders something useful
/// rather than panicking.
pub fn render(diagnostic: &Diagnostic, sources: &SourceMap, options: &RenderOptions) -> String {
    let mut groups: Vec<Group<'_>> = Vec::new();

    let mut title = level(diagnostic.severity)
        .primary_title(diagnostic.message.as_str())
        .id(diagnostic.code.as_str());
    // The code is hyperlinked to its catalogue entry with OSC 8, which is an
    // escape sequence — so it is only emitted in styled mode, keeping plain
    // output byte-comparable in golden tests and in piped `predictable check`.
    if options.color {
        title = title.id_url(diagnostic.doc_url.as_str());
    }
    let mut group = Group::with_title(title);

    // One snippet per file, spans in the order given, primary spans keeping
    // their `primary` flag so the caret lands on the right one.
    for file in files_with_spans(diagnostic) {
        if let Some(source) = sources.get(file) {
            let mut snippet = Snippet::source(source.as_str()).path(file).fold(true);
            for span in diagnostic.spans.iter().filter(|s| s.file == file) {
                let kind = if span.primary {
                    AnnotationKind::Primary
                } else {
                    AnnotationKind::Context
                };
                let mut annotation = kind.span(span.range());
                if let Some(label) = &span.label {
                    annotation = annotation.label(label.as_str());
                }
                snippet = snippet.annotation(annotation);
            }
            group = group.element(snippet);
        } else {
            group = group.element(annotate_snippets::Origin::path(file));
        }
    }

    for note in &diagnostic.notes {
        group = group.element(Level::NOTE.message(note.as_str()));
    }
    groups.push(group);

    if options.show_suggestions {
        for suggestion in &diagnostic.suggestions {
            let mut sgroup =
                Group::with_title(Level::HELP.secondary_title(suggestion.message.as_str()));
            let mut rendered_any = false;
            for file in files_with_edits(suggestion) {
                if let Some(source) = sources.get(file) {
                    let mut snippet = Snippet::source(source.as_str()).path(file).fold(true);
                    for edit in suggestion.edits.iter().filter(|e| e.file == file) {
                        snippet =
                            snippet.patch(Patch::new(edit.range(), edit.replacement.as_str()));
                    }
                    sgroup = sgroup.element(snippet);
                    rendered_any = true;
                }
            }
            let _ = rendered_any;
            groups.push(sgroup);
        }
    }

    let renderer = if options.color {
        Renderer::styled()
    } else {
        Renderer::plain()
    }
    .term_width(options.term_width)
    .decor_style(if options.unicode {
        DecorStyle::Unicode
    } else {
        DecorStyle::Ascii
    });

    renderer.render(&groups)
}

/// Render a whole batch, separated by a blank line, in the order given.
pub fn render_all(
    diagnostics: &[Diagnostic],
    sources: &SourceMap,
    options: &RenderOptions,
) -> String {
    diagnostics
        .iter()
        .map(|d| render(d, sources, options))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn files_with_spans(diagnostic: &Diagnostic) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for span in &diagnostic.spans {
        if !out.contains(&span.file.as_str()) {
            out.push(span.file.as_str());
        }
    }
    out
}

fn files_with_edits(suggestion: &crate::model::Suggestion) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for edit in &suggestion.edits {
        if !out.contains(&edit.file.as_str()) {
            out.push(edit.file.as_str());
        }
    }
    out
}
