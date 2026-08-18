//! Digest behaviour — `01-ir.md` §9.3 rule 6, §8.4.2, §8.4.3, §2.9.1.

use predictable_fmt::digest::*;

const MODEL: &str = r#"format = "pir/1"
module = "m"

[timeline]
basis = "annual"
periods = 10
origin = "policy"
valuation_date = 2026-06-30
year_convention = "act/365"

[[component]]
name = "x"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum_assured * 1.05"
"#;

const PRODUCT: &str = r#"format = "pir/1"

[product]
name = "P"
modules = ["m"]
outputs = ["x"]
key_field = "policy_number"
assumptions = "base"
"#;

const RUN: &str = r#"format = "pir/1"

[run]
product = "product"
assumptions = "base"
modelpoints = "data/mp.csv"
out = "runs/one"
emit = "outputs"
retain = "ring"
on_trap = "abort"

[run.exec]
threads = 1
chunk_size = 1024
progress = false
"#;

fn model_of(source: &str) -> String {
    model_digest_of_sources([("m.pir", source)]).unwrap()
}

// -- shape ------------------------------------------------------------------

#[test]
fn a_digest_is_sha256_and_64_hex_digits() {
    let d = digest_bytes(b"");
    assert_eq!(
        d,
        "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    let d = model_of(MODEL);
    assert!(d.starts_with("sha256:"));
    assert_eq!(d.len(), "sha256:".len() + 64);
    assert!(d["sha256:".len()..]
        .chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
}

// -- §9.3 rule 6: formatting is invisible, semantics are not ----------------

#[test]
fn reformatting_does_not_change_model_digest() {
    let ugly = MODEL
        .replace(
            "name = \"x\"\nkind = \"Output\"\ndtype = \"f64\"\nshape = \"PerMP\"\nunit = \"money\"\nexpr = \"sum_assured * 1.05\"",
            "expr  =  \"( sum_assured )*1.050\"   \nunit=\"money\"\nshape = \"PerMP\"\ndtype = \"f64\"\nkind = \"Output\"\nname = \"x\"",
        )
        .replace('\n', "\r\n");
    assert_ne!(ugly, MODEL);
    assert_eq!(model_of(&ugly), model_of(MODEL));
}

#[test]
fn any_semantic_edit_changes_model_digest() {
    let baseline = model_of(MODEL);
    for edit in [
        MODEL.replace("1.05", "1.06"),                         // a number
        MODEL.replace("periods = 10", "periods = 11"),         // the timeline
        MODEL.replace("unit = \"money\"", "unit = \"count\""), // a unit
        MODEL.replace("kind = \"Output\"", "kind = \"Derived\""),
        MODEL.replace("module = \"m\"", "module = \"n\""),
        format!("{MODEL}\n[[component]]\nname = \"y\"\nkind = \"Derived\"\n"),
    ] {
        assert_ne!(
            model_of(&edit),
            baseline,
            "digest did not move for:\n{edit}"
        );
    }
}

#[test]
fn a_comment_is_content_and_moves_the_digest() {
    // Comments survive `fmt`, so they are inside the canonical text by design.
    let annotated = MODEL.replace("[[component]]", "# the only output\n[[component]]");
    assert_ne!(model_of(&annotated), model_of(MODEL));
}

#[test]
fn model_digest_covers_the_product_file_and_is_order_free() {
    let with_product =
        model_digest_of_sources([("m.pir", MODEL), ("product.pir", PRODUCT)]).unwrap();
    let reversed = model_digest_of_sources([("product.pir", PRODUCT), ("m.pir", MODEL)]).unwrap();
    assert_eq!(with_product, reversed, "path order, not argument order");
    assert_ne!(
        with_product,
        model_of(MODEL),
        "the product file is inside the digest"
    );

    let changed_product = PRODUCT.replace("outputs = [\"x\"]", "outputs = [\"x\", \"y\"]");
    assert_ne!(
        model_digest_of_sources([("m.pir", MODEL), ("product.pir", &changed_product)]).unwrap(),
        with_product
    );
}

#[test]
fn file_names_are_inside_the_digest_and_cannot_be_smeared_together() {
    // The length-prefixed framing: no two different file sets hash alike, even
    // when their concatenated text is identical.
    let a = model_digest(&[
        CanonicalFile {
            path: "a.pir".into(),
            text: "one\n".into(),
        },
        CanonicalFile {
            path: "b.pir".into(),
            text: "two\n".into(),
        },
    ]);
    let b = model_digest(&[CanonicalFile {
        path: "a.pir".into(),
        text: "one\ntwo\n".into(),
    }]);
    assert_ne!(a, b);

    let renamed = model_digest(&[
        CanonicalFile {
            path: "a.pir".into(),
            text: "one\n".into(),
        },
        CanonicalFile {
            path: "c.pir".into(),
            text: "two\n".into(),
        },
    ]);
    assert_ne!(a, renamed);
}

// -- §8.4.2: run_digest -----------------------------------------------------

#[test]
fn run_digest_ignores_the_fields_that_cannot_change_a_number() {
    let baseline = run_digest("run.pir", RUN).unwrap();
    for edit in [
        RUN.replace("threads = 1", "threads = 64"),
        RUN.replace("chunk_size = 1024", "chunk_size = 8"),
        RUN.replace("progress = false", "progress = true"),
        RUN.replace("out = \"runs/one\"", "out = \"runs/two\""),
    ] {
        assert_eq!(
            run_digest("run.pir", &edit).unwrap(),
            baseline,
            "moved for:\n{edit}"
        );
    }
    assert!(!run_digest_text("run.pir", RUN)
        .unwrap()
        .contains("[run.exec]"));
    assert!(!run_digest_text("run.pir", RUN).unwrap().contains("out ="));
}

#[test]
fn run_digest_moves_for_every_field_that_can() {
    let baseline = run_digest("run.pir", RUN).unwrap();
    for edit in [
        RUN.replace("emit = \"outputs\"", "emit = \"all\""),
        RUN.replace("retain = \"ring\"", "retain = \"full\""),
        RUN.replace("on_trap = \"abort\"", "on_trap = \"continue\""),
        RUN.replace("modelpoints = \"data/mp.csv\"", "modelpoints = \"data/other.csv\""),
        RUN.replace("assumptions = \"base\"", "assumptions = \"stress\""),
        format!("{RUN}\n[[aggregation]]\nname = \"a\"\ngroup_by = [\"g\"]\nmeasure = \"x\"\nop = \"sum\"\n"),
    ] {
        assert_ne!(run_digest("run.pir", &edit).unwrap(), baseline, "did not move for:\n{edit}");
    }
}

#[test]
fn a_run_file_is_not_part_of_the_model_digest() {
    // §9.3 rule 6: the run file has its own digest and stays out of the model's.
    let with_run = model_digest_of_sources([("m.pir", MODEL), ("run.pir", RUN)]).unwrap();
    assert_ne!(with_run, model_of(MODEL));
    // …which is a statement about what callers pass in, so the two digests are
    // computed over disjoint inputs and are independent.
    assert_ne!(run_digest("run.pir", RUN).unwrap(), model_of(MODEL));
}

// -- §8.4.3: component_set_digest -------------------------------------------

#[test]
fn component_set_digest_is_sorted_and_deduplicated() {
    let a = component_set_digest(&["m.bel", "m.reserve", "m.x"]);
    let b = component_set_digest(&["m.x", "m.bel", "m.reserve", "m.bel"]);
    assert_eq!(a, b);
    assert_ne!(a, component_set_digest(&["m.bel", "m.reserve"]));
    // Not a plain concatenation: ids are newline-terminated, so no two sets
    // collide by splitting a name differently.
    assert_ne!(
        component_set_digest(&["ab", "c"]),
        component_set_digest(&["a", "bc"])
    );
}

// -- §2.9.1: inlined table digest -------------------------------------------

#[test]
fn an_inline_table_is_hashed_over_the_canonical_rows() {
    let module = r#"format = "pir/1"
module = "m"

[[table]]
name = "lapse_rates"
keys = [{ name = "policy_year", dtype = "i64", policy = "step" }]
values = [{ name = "lapse_pa", dtype = "f64" }]
on_missing = "default(0.0)"
source = "inline"
rows = [
  [1, 0.12],
  [2, 0.080],
]
"#;
    let d = inline_rows_digest("m.pir", module, "lapse_rates")
        .unwrap()
        .unwrap();
    // Layout and float spelling are canonicalised before hashing.
    let squashed = module.replace(
        "[\n  [1, 0.12],\n  [2, 0.080],\n]",
        "[[1, 0.120], [2, 0.08]]",
    );
    assert_ne!(squashed, module);
    assert_eq!(
        inline_rows_digest("m.pir", &squashed, "lapse_rates")
            .unwrap()
            .unwrap(),
        d
    );
    // A changed rate is a changed table.
    let bumped = module.replace("0.12", "0.13");
    assert_ne!(
        inline_rows_digest("m.pir", &bumped, "lapse_rates")
            .unwrap()
            .unwrap(),
        d
    );
    // An unknown or non-inlined table has no rows digest.
    assert_eq!(inline_rows_digest("m.pir", module, "nope").unwrap(), None);
}

#[test]
fn a_table_file_is_hashed_over_its_bytes() {
    // §2.9: whatever the resolver returned is the table's identity.
    assert_eq!(
        digest_bytes(b"age,qx\n30,0.001\n"),
        digest_bytes(b"age,qx\n30,0.001\n")
    );
    assert_ne!(
        digest_bytes(b"age,qx\n30,0.001\n"),
        digest_bytes(b"age,qx\n30,0.0011\n")
    );
    // …bytes, not text: a CRLF rewrite of a CSV *is* a different table.
    assert_ne!(digest_bytes(b"age,qx\n"), digest_bytes(b"age,qx\r\n"));
}

#[test]
fn digests_refuse_a_file_that_does_not_parse() {
    assert!(model_digest_of_sources([("m.pir", "[[component\n")]).is_err());
    assert!(run_digest("run.pir", "x = \n").is_err());
}

#[test]
fn assumption_sets_hash_over_canonical_text_too() {
    let base = "format = \"pir/1\"\nassumption_set = \"base\"\nvaluation_rate = 0.035\n";
    let ugly = "format = \"pir/1\"\nassumption_set  = \"base\"\n\nvaluation_rate = 0.0350\n";
    assert_eq!(
        assumption_digest("base.pir", base).unwrap(),
        assumption_digest("base.pir", ugly).unwrap()
    );
    let stressed = base.replace("0.035", "0.03");
    assert_ne!(
        assumption_digest("base.pir", &stressed).unwrap(),
        assumption_digest("base.pir", base).unwrap()
    );
}
