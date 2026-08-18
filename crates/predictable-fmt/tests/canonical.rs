//! Canonical-form rules, one test per rule of `01-ir.md` §4.1.

use predictable_fmt::{format_expression, format_source, is_canonical};

fn component(body: &str) -> String {
    format!("format = \"pir/1\"\nmodule = \"m\"\n\n[[component]]\n{body}")
}

/// Format a component body and give back only its `[[component]]` block.
fn fmt_component(body: &str) -> String {
    let out = format_source("m.pir", &component(body)).expect("formats");
    out.split_once("[[component]]\n").unwrap().1.to_string()
}

fn fmt_expr(expr: &str) -> String {
    let body = format!(
        "name = \"x\"\nkind = \"Derived\"\nexpr = \"{}\"\n",
        expr.replace('"', "\\\"")
    );
    let out = fmt_component(&body);
    let line = out.lines().find(|l| l.starts_with("expr = ")).unwrap();
    line["expr = \"".len()..line.len() - 1].replace("\\\"", "\"")
}

// -- rule 1: intra-component key order --------------------------------------

#[test]
fn component_keys_take_the_fixed_order() {
    let scrambled = "\
output = true
doc = \"d\"
expr = \"a\"
init = \"0.0\"
timing = \"start\"
unit = \"money\"
shape = \"Series\"
dtype = \"f64\"
kind = \"Derived\"
name = \"x\"
tags = [\"one\"]
";
    assert_eq!(
        fmt_component(scrambled),
        "\
name = \"x\"
kind = \"Derived\"
dtype = \"f64\"
shape = \"Series\"
unit = \"money\"
timing = \"start\"
init = \"0.0\"
expr = \"a\"
doc = \"d\"
tags = [\"one\"]
output = true
"
    );
}

#[test]
fn unknown_component_keys_keep_their_order_after_the_known_ones() {
    let out = fmt_component("zeta = 1\nname = \"x\"\nalpha = 2\nkind = \"Derived\"\n");
    assert_eq!(
        out,
        "name = \"x\"\nkind = \"Derived\"\nzeta = 1\nalpha = 2\n"
    );
}

// -- rule 2: declaration order is never touched -----------------------------

#[test]
fn blocks_and_components_keep_declaration_order() {
    let source = "\
format = \"pir/1\"
module = \"m\"

[[component]]
name = \"zulu\"
kind = \"Derived\"

[[component]]
name = \"alpha\"
kind = \"Derived\"

[timeline]
basis = \"annual\"
";
    let out = format_source("m.pir", source).unwrap();
    let order: Vec<&str> = out
        .lines()
        .filter(|l| l.starts_with("name = ") || l.starts_with('['))
        .collect();
    assert_eq!(
        order,
        [
            "[[component]]",
            "name = \"zulu\"",
            "[[component]]",
            "name = \"alpha\"",
            "[timeline]"
        ]
    );
}

// -- rule 3: expression normalisation ---------------------------------------

#[test]
fn binary_operators_get_single_spaces() {
    assert_eq!(fmt_expr("a*b   +c"), "a * b + c");
    assert_eq!(fmt_expr("a>=b and c!=d"), "a >= b and c != d");
    assert_eq!(fmt_expr("a^b"), "a ^ b");
    assert_eq!(fmt_expr("-a"), "-a");
    assert_eq!(fmt_expr("not  a"), "not a");
    assert_eq!(fmt_expr("sum( a ,b )"), "sum(a, b)");
    assert_eq!(fmt_expr("tbl@( a ,b )"), "tbl@(a, b)");
    assert_eq!(fmt_expr("if a then b else c"), "if a then b else c");
    assert_eq!(fmt_expr("x[t-1]"), "x[t-1]");
    assert_eq!(fmt_expr("x[ 3 ]"), "x[3]");
    assert_eq!(fmt_expr("retime(x, mid)"), "retime(x, mid)");
}

#[test]
fn redundant_parentheses_are_removed() {
    assert_eq!(fmt_expr("(a)"), "a");
    assert_eq!(fmt_expr("((a + b))"), "a + b");
    assert_eq!(fmt_expr("(sum(x))"), "sum(x)");
    assert_eq!(fmt_expr("(a * b) * c"), "a * b * c");
    // §4.1 rule 3 names only mixed `*`/`+`; `and` under `or` is not exempt.
    assert_eq!(fmt_expr("a or (b and c)"), "a or b and c");
    assert_eq!(fmt_expr("not (a)"), "not a");
}

#[test]
fn parentheses_that_carry_meaning_survive() {
    assert_eq!(fmt_expr("(a + b) * c"), "(a + b) * c");
    assert_eq!(fmt_expr("a - (b - c)"), "a - (b - c)");
    // Right-hand grouping at equal precedence is *not* redundant: floating
    // point addition is not associative and §9.1 fixes evaluation order.
    assert_eq!(fmt_expr("a + (b + c)"), "a + (b + c)");
    assert_eq!(fmt_expr("a * (b * c)"), "a * (b * c)");
    assert_eq!(fmt_expr("a / (b * c)"), "a / (b * c)");
    assert_eq!(fmt_expr("(a or b) and c"), "(a or b) and c");
    assert_eq!(fmt_expr("not (a > b)"), "not (a > b)");
    assert_eq!(fmt_expr("-(a + b)"), "-(a + b)");
    assert_eq!(fmt_expr("(a + b) ^ 2"), "(a + b) ^ 2");
    assert_eq!(
        fmt_expr("if (a) then (b + c) * d else e"),
        "if a then (b + c) * d else e"
    );
}

#[test]
fn mixed_precedence_parentheses_are_preserved_as_written_and_never_invented() {
    // Preserved when the author wrote them …
    assert_eq!(fmt_expr("(a * b) + c"), "(a * b) + c");
    assert_eq!(fmt_expr("a - (b / c)"), "a - (b / c)");
    // … and never added when the author did not.
    assert_eq!(fmt_expr("a * b + c"), "a * b + c");
    assert_eq!(fmt_expr("a - b / c"), "a - b / c");
    // The rule is about `*`/`+` only: comparison operands are left bare.
    assert_eq!(fmt_expr("(a + b) < c"), "a + b < c");
}

#[test]
fn expression_normalisation_is_idempotent() {
    for e in [
        "(a * b) + c",
        "a*b+c",
        "if x then (a * b) + c else -(d)",
        "npv( premium ,  disc )",
        "sa8990@(age,gender)*loading",
        "a ^ b ^ c",
        "a - (b - c)",
    ] {
        let once = format_expression(e).0;
        let twice = format_expression(&once).0;
        assert_eq!(once, twice, "not a fixed point: {e}");
    }
}

#[test]
fn a_broken_expression_is_left_alone_and_reported() {
    let (text, diags) = format_expression("a +");
    assert_eq!(text, "a +");
    assert!(diags.iter().any(|d| d.is_error()));
    assert!(format_source(
        "m.pir",
        &component("name = \"x\"\nkind = \"Derived\"\nexpr = \"a +\"\n")
    )
    .is_err());
}

// -- rule 4: shortest float -------------------------------------------------

#[test]
fn floats_are_shortest_round_trip_everywhere_they_appear() {
    let source = "\
format = \"pir/1\"
module = \"m\"

[[solve]]
tolerance = 1.0e-8
bracket = [0.0, 1000000.0]
scale = 1.050
noisy = 0.10000000000000001
whole = 45.00
";
    let out = format_source("m.pir", source).unwrap();
    assert!(out.contains("tolerance = 1e-8"), "{out}");
    assert!(out.contains("bracket = [0.0, 1000000.0]"), "{out}");
    assert!(out.contains("scale = 1.05"), "{out}");
    assert!(out.contains("noisy = 0.1"), "{out}");
    assert!(out.contains("whole = 45.0"), "{out}");
    // Inside expressions too.
    assert_eq!(
        fmt_expr("x * 1.050 + 0.10000000000000001"),
        "x * 1.05 + 0.1"
    );
    // Integers stay integers: `1` is not `1.0`.
    assert_eq!(fmt_expr("x * 1"), "x * 1");
}

// -- rule 5: UTF-8, LF, one trailing newline, no trailing whitespace ---------

#[test]
fn line_endings_and_trailing_whitespace_are_normalised() {
    let source = "format = \"pir/1\"\r\nmodule = \"m\"   \r\n\r\n\r\n";
    let out = format_source("m.pir", source).unwrap();
    assert_eq!(out, "format = \"pir/1\"\nmodule = \"m\"\n");
}

#[test]
fn a_missing_final_newline_is_added() {
    let out = format_source("m.pir", "format = \"pir/1\"").unwrap();
    assert_eq!(out, "format = \"pir/1\"\n");
}

#[test]
fn non_ascii_text_survives_unharmed() {
    let source = "format = \"pir/1\"\ndoc = \"réserve — 保険 ✓\"\n";
    assert!(is_canonical("m.pir", source).unwrap());
}

#[test]
fn quotes_and_escapes_round_trip() {
    let source = "format = \"pir/1\"\ndoc = \"say \\\"act/365\\\" twice\\nand a tab\\there\"\n";
    let out = format_source("m.pir", source).unwrap();
    assert_eq!(out, source);
}

// -- layout -----------------------------------------------------------------

#[test]
fn one_key_per_line_and_one_blank_line_before_each_header() {
    let source = "\
format = \"pir/1\"
[[modelpoint_field]]
name = \"policy_number\"; dtype = \"str\"; required = true


[[component]]
name = \"x\"
";
    let out = format_source("m.pir", source).unwrap();
    assert_eq!(
        out,
        "\
format = \"pir/1\"

[[modelpoint_field]]
name = \"policy_number\"
dtype = \"str\"
required = true

[[component]]
name = \"x\"
"
    );
}

#[test]
fn comments_are_kept_above_the_item_they_precede() {
    let source = "\
format = \"pir/1\"

# A table, resolved from the filesystem.
[[table]]
name = \"t\"
# the key list matters
keys = [{ name = \"age\", dtype = \"i64\" }]
";
    let out = format_source("m.pir", source).unwrap();
    assert_eq!(out, source);
}

#[test]
fn comments_inside_a_component_are_hoisted_under_its_header() {
    // The keys below move, so a comment cannot stay glued to one of them.
    let source = "\
format = \"pir/1\"

[[component]]
# survivorship
expr = \"a\"
name = \"x\"
";
    let out = format_source("m.pir", source).unwrap();
    assert_eq!(
        out,
        "format = \"pir/1\"\n\n[[component]]\n# survivorship\nname = \"x\"\nexpr = \"a\"\n"
    );
    assert!(is_canonical("m.pir", &out).unwrap());
}

#[test]
fn arrays_of_composites_go_one_element_per_line() {
    let source = "\
format = \"pir/1\"

[[table]]
name = \"t\"
keys = [{ name = \"age\", dtype = \"i64\", policy = \"clamp\" }, { name = \"sex\", dtype = \"str\", policy = \"exact\" }]
values = [{ name = \"qx\", dtype = \"f64\" }]
labels = [\"a\", \"b\"]
empty = []
rows = [[1, 0.12], [2, 0.08]]
";
    let out = format_source("m.pir", source).unwrap();
    assert_eq!(
        out,
        "\
format = \"pir/1\"

[[table]]
name = \"t\"
keys = [
  { name = \"age\", dtype = \"i64\", policy = \"clamp\" },
  { name = \"sex\", dtype = \"str\", policy = \"exact\" },
]
values = [{ name = \"qx\", dtype = \"f64\" }]
labels = [\"a\", \"b\"]
empty = []
rows = [
  [1, 0.12],
  [2, 0.08],
]
"
    );
}

#[test]
fn dates_are_kept_verbatim() {
    let source = "format = \"pir/1\"\n\n[timeline]\nvaluation_date = 2026-06-30\n";
    assert!(is_canonical("m.pir", source).unwrap());
}

#[test]
fn fmt_refuses_a_file_that_does_not_parse() {
    let err = format_source("m.pir", "format = \n[[component\n").unwrap_err();
    assert!(err.diagnostics.iter().any(|d| d.is_error()));
    assert!(err.to_string().contains("cannot format"));
}
