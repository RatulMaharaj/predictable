//! `pir.json` must be lossless: every construct of §2 survives encode → decode unchanged, and
//! re-encoding the decoded value produces byte-identical text.

mod common;

use common::*;
use predictable_ir::*;
use serde_json::Value;

fn roundtrip(file: PirFile) -> PirFile {
    let text = file.to_json_pretty().expect("encode");
    let back = PirFile::from_json(&text).expect("decode");
    assert_eq!(file, back, "decoded value differs from the original");
    let text2 = back.to_json_pretty().expect("re-encode");
    assert_eq!(text, text2, "re-encoding is not byte-identical");
    back
}

#[test]
fn schema_module_roundtrips() {
    roundtrip(PirFile::from(schema_module()));
}

#[test]
fn model_module_roundtrips() {
    roundtrip(PirFile::from(model_module()));
}

#[test]
fn product_file_roundtrips() {
    roundtrip(PirFile::from(product_file()));
}

#[test]
fn run_file_roundtrips() {
    roundtrip(PirFile::from(run_file()));
}

#[test]
fn assumption_set_roundtrips() {
    let mut values = std::collections::BTreeMap::new();
    values.insert("valuation_rate".to_string(), LitValue::Float(0.035));
    values.insert("mortality_loading".to_string(), LitValue::Float(1.0));
    values.insert("max_age".to_string(), LitValue::Int(120));
    values.insert("use_select".to_string(), LitValue::Bool(true));
    values.insert(
        "basis_label".to_string(),
        LitValue::Text("statutory".into()),
    );
    let set = AssumptionSet {
        format: FORMAT.into(),
        assumption_set: "base".into(),
        model_module: "term_assurance".into(),
        values,
    };
    let back = roundtrip(PirFile::from(set.clone()));
    match back {
        PirFile::AssumptionSet(a) => assert_eq!(a.values, set.values),
        other => panic!("wrong file kind: {}", other.kind_name()),
    }
}

#[test]
fn every_expr_node_variant_roundtrips() {
    let nodes = vec![
        Expr::Lit {
            dtype: DType::F64,
            value: LitValue::Float(1.05),
        },
        Expr::Lit {
            dtype: DType::I64,
            value: LitValue::Int(-3),
        },
        Expr::Lit {
            dtype: DType::Bool,
            value: LitValue::Bool(false),
        },
        Expr::Lit {
            dtype: DType::Date,
            value: LitValue::Text("2026-06-30".into()),
        },
        Expr::Lit {
            dtype: DType::Enum("Gender".into()),
            value: LitValue::Text("F".into()),
        },
        Expr::r#ref("premium"),
        Expr::Lag {
            name: "reserve".into(),
            k: 3,
        },
        Expr::At {
            name: "premium".into(),
            k: 0,
        },
        Expr::Unary {
            op: UnaryOp::Neg,
            operand: Box::new(Expr::r#ref("x")),
        },
        Expr::Unary {
            op: UnaryOp::Not,
            operand: Box::new(Expr::r#ref("flag")),
        },
        Expr::binary(BinaryOp::Pow, Expr::r#ref("x"), Expr::f64(2.0)),
        Expr::If {
            cond: Box::new(Expr::binary(BinaryOp::Eq, Expr::r#ref("x"), Expr::f64(0.0))),
            then: Box::new(Expr::f64(0.0)),
            otherwise: Box::new(Expr::binary(
                BinaryOp::Div,
                Expr::f64(1.0),
                Expr::r#ref("x"),
            )),
        },
        Expr::Call {
            func: "round".into(),
            args: vec![
                Expr::r#ref("x"),
                Expr::Lit {
                    dtype: DType::I64,
                    value: LitValue::Int(2),
                },
            ],
        },
        Expr::Lookup {
            table: "sa8990".into(),
            keys: vec![Expr::r#ref("age"), Expr::r#ref("gender")],
        },
        Expr::Agg {
            op: AggOp::CountWhile,
            value: Box::new(Expr::r#ref("in_term")),
            pred: None,
        },
        Expr::Agg {
            op: AggOp::Npv,
            value: Box::new(Expr::r#ref("premium_income")),
            pred: Some(Box::new(Expr::r#ref("disc_factor"))),
        },
    ];

    for node in nodes {
        let text = serde_json::to_string(&node).unwrap();
        let back: Expr = serde_json::from_str(&text).unwrap();
        assert_eq!(node, back, "expr node did not round-trip: {text}");
        // Every node is self-describing.
        let v: Value = serde_json::from_str(&text).unwrap();
        assert!(v.get("node").is_some(), "missing node tag in {text}");
    }
}

#[test]
fn all_binary_and_agg_ops_roundtrip() {
    let binaries = [
        BinaryOp::Add,
        BinaryOp::Sub,
        BinaryOp::Mul,
        BinaryOp::Div,
        BinaryOp::Pow,
        BinaryOp::Eq,
        BinaryOp::Ne,
        BinaryOp::Lt,
        BinaryOp::Le,
        BinaryOp::Gt,
        BinaryOp::Ge,
        BinaryOp::And,
        BinaryOp::Or,
    ];
    for op in binaries {
        let e = Expr::binary(op, Expr::r#ref("a"), Expr::r#ref("b"));
        let back: Expr = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(e, back);
        assert!(!op.symbol().is_empty());
    }

    let aggs = [
        AggOp::Sum,
        AggOp::SumKahan,
        AggOp::Npv,
        AggOp::First,
        AggOp::Last,
        AggOp::At,
        AggOp::MaxOver,
        AggOp::MinOver,
        AggOp::CountWhile,
    ];
    for op in aggs {
        let e = Expr::Agg {
            op,
            value: Box::new(Expr::r#ref("x")),
            pred: None,
        };
        let back: Expr = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(e, back);
    }
}

#[test]
fn dtype_and_unit_spellings_are_exactly_the_spec() {
    let dtypes = [
        ("f64", DType::F64),
        ("i64", DType::I64),
        ("bool", DType::Bool),
        ("date", DType::Date),
        ("str", DType::Str),
        ("enum(Gender)", DType::Enum("Gender".into())),
    ];
    for (text, dtype) in dtypes {
        assert_eq!(dtype.to_string(), text);
        assert_eq!(text.parse::<DType>().unwrap(), dtype);
        assert_eq!(
            serde_json::to_string(&dtype).unwrap(),
            format!("\"{text}\"")
        );
    }

    let units = [
        ("none", Unit::None),
        ("money", Unit::Money),
        ("rate(annual)", Unit::Rate(RateBasis::Annual)),
        ("rate(monthly)", Unit::Rate(RateBasis::Monthly)),
        ("rate(period)", Unit::Rate(RateBasis::Period)),
        ("prob", Unit::Prob),
        ("count", Unit::Count),
        ("years", Unit::Years),
        ("months", Unit::Months),
        ("factor", Unit::Factor),
    ];
    for (text, unit) in units {
        assert_eq!(unit.to_string(), text);
        assert_eq!(text.parse::<Unit>().unwrap(), unit);
    }
}

#[test]
fn kind_shape_and_timing_spellings_are_exactly_the_spec() {
    let kinds = [
        ("Input.Modelpoint", Kind::InputModelpoint),
        ("Input.Assumption", Kind::InputAssumption),
        ("Input.Table", Kind::InputTable),
        ("Input.Timeline", Kind::InputTimeline),
        ("Derived", Kind::Derived),
        ("Output", Kind::Output),
    ];
    for (text, kind) in kinds {
        assert_eq!(serde_json::to_string(&kind).unwrap(), format!("\"{text}\""));
        assert_eq!(kind.to_string(), text);
    }

    for (text, shape) in [
        ("Scalar", Shape::Scalar),
        ("PerMP", Shape::PerMp),
        ("Series", Shape::Series),
    ] {
        assert_eq!(
            serde_json::to_string(&shape).unwrap(),
            format!("\"{text}\"")
        );
    }

    for (text, timing) in [
        ("start", Timing::Start),
        ("end", Timing::End),
        ("mid", Timing::Mid),
        ("point", Timing::Point),
    ] {
        assert_eq!(
            serde_json::to_string(&timing).unwrap(),
            format!("\"{text}\"")
        );
    }
}

#[test]
fn bad_dtype_and_unit_strings_are_rejected_with_a_helpful_message() {
    let err = "f32".parse::<DType>().unwrap_err();
    assert!(err.to_string().contains("is not a dtype"), "{err}");
    assert!("decimal".parse::<DType>().is_err());
    assert!("enum()".parse::<DType>().is_err());

    let err = "rate(daily)".parse::<Unit>().unwrap_err();
    assert!(err.to_string().contains("is not a unit"), "{err}");
    assert!("currency".parse::<Unit>().is_err());
    assert!("rate".parse::<Unit>().is_err());
}

#[test]
fn unknown_file_kind_is_an_error_not_a_silent_empty_module() {
    let err = PirFile::from_json(r#"{"format": "pir/1"}"#).unwrap_err();
    assert!(matches!(err, IrError::UnknownFileKind), "{err}");
    assert!(PirFile::from_json("[]").is_err());
}

#[test]
fn a_foreign_format_version_is_refused() {
    let text = r#"{"format": "pir/2", "module": "x"}"#;
    let file = PirFile::from_json(text).unwrap();
    let err = file.check_format().unwrap_err();
    assert!(matches!(err, IrError::Format { .. }), "{err}");
    assert!(err.to_string().contains("pir/1"));

    assert!(PirFile::from(Module::new("x")).check_format().is_ok());
}

#[test]
fn file_kind_is_decided_by_the_top_level_key() {
    for (json, kind) in [
        (
            serde_json::to_string(&PirFile::from(model_module())).unwrap(),
            "module",
        ),
        (
            serde_json::to_string(&PirFile::from(product_file())).unwrap(),
            "product",
        ),
        (
            serde_json::to_string(&PirFile::from(run_file())).unwrap(),
            "run",
        ),
    ] {
        assert_eq!(PirFile::from_json(&json).unwrap().kind_name(), kind);
    }
}

#[test]
fn defaults_fill_in_when_optional_keys_are_absent() {
    // A minimal hand-written run block: everything else must default per §8.4.2.
    let text = r#"{
      "format": "pir/1",
      "run": {
        "product": "products/term_uk",
        "modelpoints": "data/term.mpf.parquet",
        "out": "runs/base"
      }
    }"#;
    let file = PirFile::from_json(text).unwrap();
    let run = match file {
        PirFile::Run(r) => r,
        other => panic!("wrong kind {}", other.kind_name()),
    };
    assert_eq!(run.run.emit, Emit::Outputs);
    assert_eq!(run.run.retain, Retain::Ring);
    assert_eq!(run.run.on_trap, OnTrap::Abort);
    assert_eq!(run.run.storage_precision, StoragePrecision::F64);
    assert_eq!(run.run.max_errors, 100);
    assert!(!run.run.allow_table_drift);
    assert!(!run.run.sum_kahan);
    assert_eq!(run.run.exec.chunk_size, 1024);
    assert!(run.run.exec.progress);
    assert!(run.solves.is_empty());
    assert!(run.aggregations.is_empty());
}
