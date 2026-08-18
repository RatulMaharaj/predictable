//! `results.schema.json`: what it says, and that it says it in the shape
//! `schemas/results.schema.json` promises.

use predictable_io::outbound::*;
use predictable_ir::{Component, DType, Expr, Kind, Shape, Timing, Unit};
use serde_json::Value as J;

mod jsonschema;

fn output(name: &str, dtype: DType, shape: Shape) -> Component {
    let mut c = Component::derived(name, dtype, shape, Expr::f64(0.0));
    c.kind = Kind::Output;
    if shape == Shape::Series {
        c.timing = Some(Timing::End);
    }
    c
}

fn doc() -> ResultsSchemaDoc {
    let mut money = output("bel", DType::F64, Shape::Series);
    money.unit = Unit::Money;
    money.meta.display = Some(predictable_ir::model::Display {
        dp: Some(2),
        scale: None,
    });
    let mut agg = Component::derived(
        "pv_claims",
        DType::F64,
        Shape::PerMp,
        Expr::Agg {
            op: predictable_ir::AggOp::Sum,
            value: Box::new(Expr::r#ref("claims")),
            pred: None,
        },
    );
    agg.kind = Kind::Output;
    ResultsSchemaDoc::new(
        "all",
        vec![
            ComponentDescriptor::from_ir("term", &money),
            ComponentDescriptor::from_ir("term", &agg),
            ComponentDescriptor::from_ir("term", &output("age", DType::I64, Shape::Series)),
        ],
    )
    .unwrap()
}

#[test]
fn components_are_sorted_and_the_digest_is_order_independent() {
    let d = doc();
    let ids: Vec<&str> = d.components.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, vec!["term.age", "term.bel", "term.pv_claims"]);

    // The same set declared in another order is the same set.
    let mut reversed: Vec<ComponentDescriptor> = d.components.clone();
    reversed.reverse();
    let other = ResultsSchemaDoc::new("all", reversed).unwrap();
    assert_eq!(other.component_set_digest, d.component_set_digest);

    // A different set is a different digest — that is the emit_mismatch signal (IR §8.4.3).
    let smaller = ResultsSchemaDoc::new("outputs", d.components[..2].to_vec()).unwrap();
    assert_ne!(smaller.component_set_digest, d.component_set_digest);
}

#[test]
fn stage_and_timing_come_from_the_ir() {
    let d = doc();
    // `pv_claims` contains an Agg, so the IR calls it stage 2 and we copy that.
    assert_eq!(d.get("term.pv_claims").unwrap().stage, 2);
    assert_eq!(d.get("term.bel").unwrap().stage, 1);
    // Timing is present on Series and explicitly null on PerMP.
    assert_eq!(d.get("term.bel").unwrap().timing, Some(Timing::End));
    assert_eq!(d.get("term.pv_claims").unwrap().timing, None);
}

#[test]
fn value_column_selection_follows_dtype() {
    let d = doc();
    assert_eq!(d.get("term.bel").unwrap().value_column(), ValueColumn::F64);
    assert_eq!(d.get("term.age").unwrap().value_column(), ValueColumn::I64);
    assert_eq!(ValueColumn::F64.column_name(), "value");
    assert_eq!(ValueColumn::I64.column_name(), "value_i");
    assert_eq!(ValueColumn::Bool.column_name(), "value_b");
    assert_eq!(ValueColumn::Str.column_name(), "value_s");
    // date and enum both land in value_s (ISO-8601 / variant name).
    assert_eq!(ValueColumn::of(&DType::Date), ValueColumn::Str);
    assert_eq!(
        ValueColumn::of(&DType::Enum("Gender".into())),
        ValueColumn::Str
    );
}

#[test]
fn duplicate_ids_are_refused() {
    let c = ComponentDescriptor::from_ir("term", &output("bel", DType::F64, Shape::Series));
    let err = ResultsSchemaDoc::new("all", vec![c.clone(), c]).unwrap_err();
    assert!(matches!(err, IoOutError::DuplicateComponent(id) if id == "term.bel"));
}

#[test]
fn canonical_json_is_key_ordered_lf_and_newline_terminated() {
    let text = doc().to_canonical_json().unwrap();
    assert!(text.ends_with("}\n"));
    assert!(!text.contains('\r'));
    let keys: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("  \""))
        .map(|l| l.split('"').nth(1).unwrap())
        .collect();
    assert_eq!(
        keys,
        vec![
            "format",
            "kind",
            "emit",
            "component_set_digest",
            "columns",
            "components",
            "storage_precision"
        ]
    );
}

#[test]
fn round_trips_through_json() {
    let d = doc();
    let text = d.to_canonical_json().unwrap();
    let back: ResultsSchemaDoc = serde_json::from_str(&text).unwrap();
    let back = back.reindex().unwrap();
    assert_eq!(back.component_set_digest, d.component_set_digest);
    assert_eq!(back.components, d.components);
    assert_eq!(back.get("term.bel").unwrap().stage, 1);
}

#[test]
fn conforms_to_the_published_json_schema() {
    let schema: J = serde_json::from_str(include_str!("../schemas/results.schema.json")).unwrap();
    let instance: J = serde_json::from_str(&doc().to_canonical_json().unwrap()).unwrap();
    let errors = jsonschema::validate(&instance, &schema);
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn the_json_schema_catches_a_wrong_document() {
    let schema: J = serde_json::from_str(include_str!("../schemas/results.schema.json")).unwrap();
    let mut instance: J = serde_json::from_str(&doc().to_canonical_json().unwrap()).unwrap();
    // Q15: f32 storage is not a thing in IR 1.0.
    instance["storage_precision"] = J::String("f32".into());
    // And a shape the lattice does not have.
    instance["components"][0]["shape"] = J::String("Matrix".into());
    let errors = jsonschema::validate(&instance, &schema);
    assert_eq!(errors.len(), 2, "{errors:#?}");
}

#[test]
fn storage_precision_is_always_f64() {
    // Q15: T14 writes `double` unconditionally and says so in the document.
    assert_eq!(doc().storage_precision, "f64");
}
