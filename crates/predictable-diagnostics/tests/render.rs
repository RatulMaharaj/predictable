//! Terminal rendering.

use predictable_diagnostics::{
    render, render_all, Diagnostic, Edit, RenderOptions, SourceMap, Span, Suggestion,
};

const TERM: &str =
    "format = \"pir/1\"\n\n[[component]]\nname = \"reserve\"\nexpr = \"bel * 1.05\"\n";

fn sources() -> SourceMap {
    let mut map = SourceMap::new();
    map.insert("term.pir", TERM);
    map
}

fn bel_span() -> Span {
    // "bel" inside the expr on line 5.
    let start = TERM.find("bel").unwrap();
    Span::primary("term.pir", start..start + 3).label("reserve reads bel at time t")
}

#[test]
fn renders_code_severity_message_and_the_underlined_source_line() {
    let diag = Diagnostic::new("E0201", "cyclic dependency in the same period").span(bel_span());
    let out = render(&diag, &sources(), &RenderOptions::default());

    assert!(out.contains("error[E0201]"), "{out}");
    assert!(
        out.contains("cyclic dependency in the same period"),
        "{out}"
    );
    assert!(
        out.contains("term.pir:5:9"),
        "origin should carry line:col\n{out}"
    );
    assert!(out.contains("expr = \"bel * 1.05\""), "{out}");
    assert!(
        out.contains("^^^"),
        "primary span should be underlined\n{out}"
    );
    assert!(out.contains("reserve reads bel at time t"), "{out}");
}

#[test]
fn warnings_render_as_warnings() {
    let diag = Diagnostic::new("W0101", "component `reserve` is never read").span(bel_span());
    let out = render(&diag, &sources(), &RenderOptions::default());
    assert!(out.contains("warning[W0101]"), "{out}");
    assert!(!out.contains("error["), "{out}");
}

#[test]
fn notes_are_rendered_after_the_snippet() {
    let diag = Diagnostic::new("E0201", "cyclic dependency")
        .span(bel_span())
        .note("Cycles across periods are fine; add a time lag.");
    let out = render(&diag, &sources(), &RenderOptions::default());
    let snippet_at = out.find("expr = \"bel").unwrap();
    let note_at = out.find("Cycles across periods").expect("note missing");
    assert!(
        note_at > snippet_at,
        "note should follow the snippet\n{out}"
    );
}

#[test]
fn suggestions_render_as_a_help_block_with_the_replacement_text() {
    let start = TERM.find("bel").unwrap();
    let diag = Diagnostic::new("E0201", "cyclic dependency")
        .span(bel_span())
        .suggestion(
            Suggestion::new("use last period's value").edit(Edit::replace(
                "term.pir",
                start..start + 3,
                "bel[t-1]",
            )),
        );
    let out = render(&diag, &sources(), &RenderOptions::default());
    assert!(out.contains("help: use last period's value"), "{out}");
    assert!(out.contains("bel[t-1]"), "{out}");
}

#[test]
fn suggestions_can_be_suppressed() {
    let start = TERM.find("bel").unwrap();
    let diag = Diagnostic::new("E0201", "cyclic dependency")
        .span(bel_span())
        .suggestion(
            Suggestion::new("use last period's value").edit(Edit::replace(
                "term.pir",
                start..start + 3,
                "bel[t-1]",
            )),
        );
    let options = RenderOptions {
        show_suggestions: false,
        ..RenderOptions::default()
    };
    let out = render(&diag, &sources(), &options);
    assert!(!out.contains("help:"), "{out}");
    assert!(!out.contains("bel[t-1]"), "{out}");
}

#[test]
fn secondary_spans_in_a_second_file_get_their_own_snippet() {
    let other = "name = \"bel\"\nexpr = \"reserve + ra\"\n";
    let mut map = sources();
    map.insert("bel.pir", other);
    let start = other.find("reserve").unwrap();

    let diag = Diagnostic::new("E0201", "cyclic dependency")
        .span(bel_span())
        .span(Span::secondary("bel.pir", start..start + 7).label("bel reads reserve at time t"));
    let out = render(&diag, &map, &RenderOptions::default());
    assert!(out.contains("term.pir:5"), "{out}");
    assert!(out.contains("bel.pir:2"), "{out}");
    assert!(out.contains("bel reads reserve at time t"), "{out}");
}

#[test]
fn a_span_whose_source_is_unavailable_still_renders_the_path() {
    let diag = Diagnostic::new("E0801", "table source escapes the project root")
        .span(Span::primary("../secrets/rates.csv", 0..4));
    let out = render(&diag, &SourceMap::new(), &RenderOptions::default());
    assert!(out.contains("error[E0801]"), "{out}");
    assert!(out.contains("../secrets/rates.csv"), "{out}");
}

#[test]
fn plain_rendering_has_no_ansi_escapes_and_colored_rendering_does() {
    let diag = Diagnostic::new("E0201", "cyclic dependency").span(bel_span());
    let plain = render(&diag, &sources(), &RenderOptions::default());
    assert!(
        !plain.contains('\u{1b}'),
        "plain output must be escape-free"
    );
    let colored = render(&diag, &sources(), &RenderOptions::colored());
    assert!(
        colored.contains('\u{1b}'),
        "colored output should be styled"
    );
}

#[test]
fn rendering_is_deterministic() {
    let diag = Diagnostic::new("E0201", "cyclic dependency").span(bel_span());
    let a = render(&diag, &sources(), &RenderOptions::default());
    let b = render(&diag, &sources(), &RenderOptions::default());
    assert_eq!(a, b);
}

#[test]
fn render_all_keeps_batch_order() {
    let first = Diagnostic::new("E0201", "first problem").span(bel_span());
    let second = Diagnostic::new("W0101", "second problem").span(bel_span());
    let out = render_all(&[first, second], &sources(), &RenderOptions::default());
    assert!(out.find("first problem").unwrap() < out.find("second problem").unwrap());
}

#[test]
fn a_diagnostic_with_no_spans_still_renders_its_title() {
    let diag = Diagnostic::new("E0107", "product output list does not match the model");
    let out = render(&diag, &sources(), &RenderOptions::default());
    assert!(out.contains("error[E0107]"), "{out}");
    assert!(
        out.contains("product output list does not match the model"),
        "{out}"
    );
}
