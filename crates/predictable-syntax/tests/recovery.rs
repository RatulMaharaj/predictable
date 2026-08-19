//! Error recovery and block-schema diagnostics.
//!
//! The contract this file pins down: one parse reports *every* independent
//! problem in a file, each with a code and a span, and the document still
//! contains everything that did parse.

use predictable_syntax::{parse, Diagnostics, PirDocument, Severity, SourceMap};

fn check(text: &str) -> (SourceMap, PirDocument, Diagnostics) {
    let mut sources = SourceMap::new();
    let parsed = parse(&mut sources, "model.pir", text);
    (sources, parsed.document, parsed.diagnostics)
}

const HEAD: &str = "format = \"pir/1\"\nmodule = \"m\"\n";

#[test]
fn three_broken_components_yield_three_diagnostics_and_one_good_component() {
    let text = format!(
        "{HEAD}
[[component]]
name = \"a\"
kind = \"Derived\"
dtype = \"f64\"
shape = \"Series\"
unit = \"money\"
timing = \"start\"
expr = \"b +\"

[[component]]
name = \"b\"
kind = \"Drived\"
dtype = \"f64\"
shape = \"Series\"
unit = \"money\"
timing = \"start\"
expr = \"1.0\"

[[component]]
name = \"c\"
kind = \"Derived\"
dtype = \"f64\"
shape = \"Series\"
unit = \"money\"
timming = \"start\"
expr = \"2.0\"

[[component]]
name = \"d\"
kind = \"Derived\"
dtype = \"f64\"
shape = \"Series\"
unit = \"money\"
timing = \"end\"
expr = \"a * c\"
"
    );
    let (_sources, doc, diags) = check(&text);

    // One diagnostic per problem, and parsing continued past each of them.
    assert!(
        diags.codes().contains(&"E0020"),
        "the broken expression in `a`"
    );
    assert!(
        diags.codes().contains(&"E0043"),
        "the misspelled kind in `b`"
    );
    assert!(
        diags.codes().contains(&"E0044"),
        "the misspelled key in `c`"
    );
    assert_eq!(doc.components.len(), 4, "every component still lowers");
    assert!(
        doc.component("d").unwrap().expr.is_some(),
        "the last component is unharmed"
    );

    // The misspellings come with mechanical fixes.
    let kind = diags.iter().find(|d| d.code == "E0043").unwrap();
    assert_eq!(kind.suggestions[0].replacement, "\"Derived\"");
    let key = diags.iter().find(|d| d.code == "E0044").unwrap();
    assert_eq!(key.suggestions[0].replacement, "timing");
}

#[test]
fn a_malformed_line_is_skipped_not_cascaded() {
    let text = format!(
        "{HEAD}
[[assumption]]
name = \"a\"
dtype f64
shape = \"Scalar\"

[[assumption]]
name = \"b\"
dtype = \"f64\"
shape = \"Scalar\"
"
    );
    let (_s, doc, diags) = check(&text);
    assert_eq!(
        diags.error_count(),
        1,
        "one error, not one per following line: {:?}",
        diags.codes()
    );
    assert_eq!(diags.codes(), vec!["E0004"]);
    assert_eq!(doc.assumptions.len(), 2);
}

#[test]
fn an_unclosed_header_resynchronises_on_the_next_one() {
    let text = format!(
        "{HEAD}
[[component]
name = \"a\"

[[component]]
name = \"b\"
kind = \"Derived\"
dtype = \"f64\"
shape = \"PerMP\"
unit = \"money\"
expr = \"1.0\"
"
    );
    let (_s, doc, diags) = check(&text);
    assert!(diags.codes().contains(&"E0009"));
    assert_eq!(doc.components.len(), 1);
    assert_eq!(doc.components[0].name.value, "b");
}

#[test]
fn every_diagnostic_carries_a_code_a_span_and_a_doc_url() {
    let text = format!("{HEAD}\n[[component]]\nname = \"a\"\nexpr = \"1 +\"\nnope = 1\n[[wat]]\n");
    let (sources, _doc, diags) = check(&text);
    assert!(diags.len() >= 4, "{:?}", diags.codes());
    for d in diags.iter() {
        assert_eq!(d.severity, Severity::Error);
        let span = d.primary_span().expect("a primary span");
        let loc = sources.location(span);
        assert_eq!(loc.file, "model.pir");
        assert!(loc.line >= 1 && loc.col >= 1);
        assert!(d
            .doc_url
            .starts_with("https://predictable.dev/diagnostics/"));
    }
}

#[test]
fn spans_land_on_the_right_line_and_column() {
    let text = "format = \"pir/1\"\nmodule = \"m\"\n\n[[component]]\nname = \"a\"\nkind = \"Derived\"\ndtype = \"f64\"\nshape = \"Series\"\nunit = \"money\"\ntiming = \"start\"\nexpr = \"b + \"\n";
    let (sources, _doc, diags) = check(text);
    let d = diags.iter().find(|d| d.code == "E0020").expect("E0020");
    let loc = sources.location(d.primary_span().unwrap());
    assert_eq!(loc.line, 11, "the expr line");
    assert_eq!(
        sources.line_text(d.primary_span().unwrap()),
        "expr = \"b + \""
    );
    // The span is inside the string, past `expr = "`.
    assert!(
        loc.col > 8,
        "column {} should be inside the formula",
        loc.col
    );
}

#[test]
fn duplicate_keys_and_blocks_are_errors() {
    let text = format!(
        "{HEAD}
[timeline]
basis = \"annual\"
periods = 40
periods = 50

[timeline]
basis = \"monthly\"
periods = 12
"
    );
    let (_s, doc, diags) = check(&text);
    assert!(diags.codes().contains(&"E0005"), "duplicate key");
    assert!(diags.codes().contains(&"E0047"), "duplicate block");
    assert_eq!(doc.timeline.as_ref().unwrap().periods, 40, "the first wins");
}

#[test]
fn missing_required_keys_are_named() {
    let text = format!("{HEAD}\n[[component]]\nname = \"a\"\n");
    let (_s, _doc, diags) = check(&text);
    let missing: Vec<&str> = diags
        .iter()
        .filter(|d| d.code == "E0041")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(missing.len(), 4, "kind, dtype, shape, timing: {missing:?}");
    assert!(missing.iter().any(|m| m.contains("`kind`")));
    assert!(missing.iter().any(|m| m.contains("`dtype`")));
    assert!(missing.iter().any(|m| m.contains("`shape`")));
    assert!(missing.iter().any(|m| m.contains("`timing`")));
}

#[test]
fn timing_is_rejected_on_non_series_components() {
    let text = format!(
        "{HEAD}
[[component]]
name = \"bel\"
kind = \"Output\"
dtype = \"f64\"
shape = \"PerMP\"
unit = \"money\"
timing = \"point\"
expr = \"npv(a, b)\"
"
    );
    let (_s, doc, diags) = check(&text);
    assert!(diags.codes().contains(&"E0048"));
    assert_eq!(
        doc.components[0].timing, None,
        "the tag is dropped, not half-kept"
    );
}

#[test]
fn a_derived_component_without_an_expr_is_a_hole() {
    let text = format!(
        "{HEAD}
[[component]]
name = \"a\"
kind = \"Derived\"
dtype = \"f64\"
shape = \"PerMP\"
unit = \"money\"
"
    );
    let (_s, _doc, diags) = check(&text);
    assert!(diags.codes().contains(&"E0049"), "{:?}", diags.codes());
}

#[test]
fn an_optional_modelpoint_field_must_declare_a_default() {
    let text = format!(
        "{HEAD}
[[modelpoint_field]]
name = \"bonus_rate\"; dtype = \"f64\"; unit = \"none\"

[[modelpoint_field]]
name = \"loading\"; dtype = \"f64\"; unit = \"none\"; default = 0.0
"
    );
    let (_s, doc, diags) = check(&text);
    assert_eq!(
        diags.codes(),
        vec!["E0050"],
        "only the field without a default"
    );
    assert_eq!(doc.modelpoint_fields.len(), 2);
    assert_eq!(
        doc.modelpoint_fields[1].default,
        Some(predictable_syntax::Value::Float(0.0))
    );
}

#[test]
fn a_run_may_not_set_a_timeline_field() {
    let text = "format = \"pir/1\"\n\n[run]\nproduct = \"p\"\nperiods = 480\n";
    let (_s, _doc, diags) = check(text);
    assert!(diags.codes().contains(&"E0101"), "{:?}", diags.codes());
    let d = diags.iter().find(|d| d.code == "E0101").unwrap();
    assert!(d.message.contains("periods"));
    assert_eq!(
        d.suggestions[0].replacement, "",
        "the fix is to delete the line"
    );
}

#[test]
fn f32_storage_is_refused_in_ir_1_0() {
    let text = "format = \"pir/1\"\n\n[run]\nproduct = \"p\"\nstorage_precision = \"f32\"\n";
    let (_s, _doc, diags) = check(text);
    assert!(diags.codes().contains(&"E0108"), "{:?}", diags.codes());
}

#[test]
fn an_ordered_policy_on_an_enum_key_is_refused() {
    let text = format!(
        "{HEAD}
[[table]]
name = \"t\"
keys = [{{ name = \"gender\", dtype = \"enum(Gender)\", policy = \"clamp\" }}]
values = [{{ name = \"qx\", dtype = \"f64\", unit = \"prob\" }}]
source = \"tables/t.csv\"
"
    );
    let (_s, _doc, diags) = check(&text);
    assert!(diags.codes().contains(&"E0305"), "{:?}", diags.codes());
}

#[test]
fn comments_and_blank_lines_are_ignored() {
    let text = "# a leading comment\nformat = \"pir/1\"   # trailing\nmodule = \"m\"\n\n\n# another\n[[enum]]\nname = \"Gender\"\nvalues = [\"M\", \"F\"]   # inline\n";
    let (_s, doc, diags) = check(text);
    assert!(diags.is_empty(), "{:?}", diags.codes());
    assert_eq!(doc.enums[0].values, vec!["M", "F"]);
}

#[test]
fn an_unterminated_string_is_a_lexical_error_not_a_panic() {
    let text = "format = \"pir/1\nmodule = \"m\"\n";
    let (_s, _doc, diags) = check(text);
    assert!(diags.has_errors());
}

#[test]
fn an_empty_file_parses_to_an_unknown_document() {
    let (_s, doc, diags) = check("");
    assert!(diags.is_empty());
    assert_eq!(doc.kind(), predictable_syntax::DocumentKind::Unknown);
    assert!(doc.components.is_empty());
}
