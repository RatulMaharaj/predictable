//! The `--json` serialization contract.

use predictable_diagnostics::{
    exit_code, from_json, line_col, to_json, to_json_pretty, Applicability, Diagnostic, Edit,
    Severity, SourceMap, Span, Suggestion,
};
use serde_json::Value;

fn sample() -> Diagnostic {
    Diagnostic::new("E0201", "cyclic dependency in the same period")
        .span(Span::primary("term.pir", 41..44).label("reserve reads bel at time t"))
        .span(Span::secondary("term.pir", 90..97).label("bel reads reserve at time t"))
        .note("Cycles across periods are fine.")
        .suggestion(
            Suggestion::new("read last period's value")
                .edit(Edit::replace("term.pir", 90..97, "reserve[t-1]"))
                .applicability(Applicability::MaybeIncorrect),
        )
}

#[test]
fn the_json_object_has_exactly_the_specified_keys() {
    let json: Value = serde_json::from_str(&to_json(&[sample()])).unwrap();
    let obj = json[0].as_object().unwrap();
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "code",
            "doc_url",
            "message",
            "notes",
            "severity",
            "spans",
            "suggestions"
        ]
    );
    assert_eq!(obj["code"], "E0201");
    assert_eq!(obj["severity"], "error");
    assert_eq!(
        obj["doc_url"],
        "https://predictable.dev/llm/diagnostics/#E0201"
    );
}

#[test]
fn empty_optional_fields_are_omitted() {
    let diag = Diagnostic::new("W0101", "component `unused_flag` is never read");
    let json: Value = serde_json::from_str(&to_json(&[diag])).unwrap();
    let obj = json[0].as_object().unwrap();
    assert!(!obj.contains_key("notes"), "empty notes should be omitted");
    assert!(obj["spans"].as_array().unwrap().is_empty());
    assert_eq!(obj["severity"], "warning");
}

#[test]
fn spans_and_edits_serialize_as_byte_ranges() {
    let json: Value = serde_json::from_str(&to_json(&[sample()])).unwrap();
    let span = &json[0]["spans"][0];
    assert_eq!(span["start"], 41);
    assert_eq!(span["end"], 44);
    assert_eq!(span["primary"], true);
    assert!(span.get("start_pos").is_none(), "positions are opt-in");
    let edit = &json[0]["suggestions"][0]["edits"][0];
    assert_eq!(edit["start"], 90);
    assert_eq!(edit["replacement"], "reserve[t-1]");
    assert_eq!(
        json[0]["suggestions"][0]["applicability"],
        "maybe_incorrect"
    );
}

#[test]
fn round_trips_through_json() {
    let original = vec![sample(), Diagnostic::new("N0101", "lookup key clamped")];
    let back = from_json(&to_json(&original)).unwrap();
    assert_eq!(original, back);
    assert_eq!(from_json(&to_json_pretty(&original)).unwrap(), original);
}

#[test]
fn an_empty_batch_is_an_empty_array() {
    assert_eq!(to_json(&[]), "[]");
    assert!(from_json("[]").unwrap().is_empty());
}

#[test]
fn resolved_positions_are_one_based_and_counted_in_characters() {
    let src = "format = \"pir/1\"\nexpr = \"π + bel\"\n";
    let mut sources = SourceMap::new();
    sources.insert("term.pir", src);

    // "bel" starts at byte 30, after the two-byte π on line 2 — but at character
    // column 13, which is why the byte offset is the normative one.
    let bel = src.find("bel").unwrap();
    assert_eq!(bel, 30);
    let mut diag = Diagnostic::new("E0201", "cycle").span(Span::primary("term.pir", bel..bel + 3));
    diag.resolve_positions(&sources);
    let pos = diag.spans[0].start_pos.unwrap();
    assert_eq!((pos.line, pos.column), (2, 13));

    let json: Value = serde_json::from_str(&to_json(&[diag])).unwrap();
    assert_eq!(json[0]["spans"][0]["start_pos"]["line"], 2);
}

#[test]
fn line_col_handles_edges() {
    let src = "ab\ncd";
    assert_eq!(line_col(src, 0).line, 1);
    assert_eq!(line_col(src, 0).column, 1);
    assert_eq!(line_col(src, 3).line, 2);
    assert_eq!(line_col(src, 3).column, 1);
    // Past the end clamps rather than panics.
    assert_eq!(line_col(src, 999).line, 2);
}

#[test]
fn exit_codes_follow_the_cli_contract() {
    assert_eq!(exit_code(&[]), 0);
    assert_eq!(exit_code(&[Diagnostic::new("N0101", "note")]), 0);
    assert_eq!(exit_code(&[Diagnostic::new("W0101", "lint")]), 1);
    assert_eq!(exit_code(&[Diagnostic::new("W0101", "lint"), sample()]), 2);
}

#[test]
fn severity_can_be_promoted_without_changing_the_code() {
    let promoted = Diagnostic::new("W0103", "timing mismatch").severity(Severity::Error);
    assert_eq!(promoted.code, "W0103");
    assert_eq!(exit_code(&[promoted]), 2);
}

#[test]
fn unknown_codes_are_refused() {
    assert!(Diagnostic::try_new("E9999", "nope").is_none());
}

#[test]
#[should_panic(expected = "not in the registry")]
fn constructing_an_unregistered_code_panics() {
    let _ = Diagnostic::new("E9999", "nope");
}

#[test]
fn files_lists_spans_then_edits_without_duplicates() {
    let diag = Diagnostic::new("E1501", "shadowing")
        .span(Span::primary("product.pir", 0..1))
        .span(Span::secondary("lib.pir", 0..1))
        .suggestion(Suggestion::new("add @override").edit(Edit::insert(
            "product.pir",
            0,
            "@override\n",
        )));
    assert_eq!(diag.files(), ["product.pir", "lib.pir"]);
}
