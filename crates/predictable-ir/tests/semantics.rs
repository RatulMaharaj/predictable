//! Behaviour the data model is responsible for: the shape lattice, computed `stage`, the
//! timing/null rulings, `ExprPath`, table and modelpoint policies, and the `run_digest`
//! partition of §8.4.2.

mod common;

use common::*;
use predictable_ir::*;
use serde_json::Value;

#[test]
fn shape_join_is_the_broadcast_lattice_of_2_3() {
    use Shape::*;
    let cases = [
        (Scalar, Scalar, Scalar),
        (Scalar, PerMp, PerMp),
        (Scalar, Series, Series),
        (PerMp, PerMp, PerMp),
        (PerMp, Series, Series),
        (Series, Series, Series),
    ];
    for (a, b, want) in cases {
        assert_eq!(a.join(b), want, "{a} ⊔ {b}");
        assert_eq!(b.join(a), want, "join must be commutative");
    }
    // Widening is free; narrowing is never implicit.
    assert!(!Scalar.is_narrowing_to(Series));
    assert!(Series.is_narrowing_to(PerMp));
    assert!(PerMp.is_narrowing_to(Scalar));
}

#[test]
fn stage_is_computed_from_the_expression_not_authored() {
    let m = model_module();
    // `npv(...)` makes a component stage 2 whatever its kind.
    assert_eq!(m.component("pv_premiums").unwrap().stage(), Stage::Two);
    // A component that merely *reads* a stage-2 value stays stage 1 in its own expression...
    assert_eq!(m.component("reserve").unwrap().stage(), Stage::One);
    // ...and `bel` reads two stage-2 values without an Agg of its own, so it is stage 1.
    assert_eq!(m.component("bel").unwrap().stage(), Stage::One);
    assert_eq!(m.component("num_pols_if").unwrap().stage(), Stage::One);

    // An Agg nested deep inside still promotes.
    let deep = Component::derived(
        "x",
        DType::F64,
        Shape::PerMp,
        add(
            Expr::f64(1.0),
            Expr::Call {
                func: "abs".into(),
                args: vec![Expr::Agg {
                    op: AggOp::Sum,
                    value: Box::new(Expr::r#ref("y")),
                    pred: None,
                }],
            },
        ),
    );
    assert_eq!(deep.stage(), Stage::Two);

    // `stage` is exposed as 1/2 in JSON for consumers that never re-derive it.
    assert_eq!(serde_json::to_string(&Stage::Two).unwrap(), "2");
    assert_eq!(serde_json::from_str::<Stage>("1").unwrap(), Stage::One);
    assert!(serde_json::from_str::<Stage>("3").is_err());
}

#[test]
fn timing_is_null_on_untimed_components_and_never_omitted() {
    // Q14: a Scalar/PerMP component has no timing, and that fact is written as `null`.
    let pv = per_mp_output("pv", Unit::Money, Expr::r#ref("x"));
    let v: Value = serde_json::to_value(&pv).unwrap();
    assert_eq!(v.get("timing"), Some(&Value::Null));

    let series_component = model_module().component("qx").unwrap().clone();
    let v: Value = serde_json::to_value(&series_component).unwrap();
    assert_eq!(v.get("timing").and_then(Value::as_str), Some("end"));
}

#[test]
fn npv_discount_exponent_follows_the_timing_tag() {
    assert_eq!(Timing::Start.discount_exponent(4), 4.0);
    assert_eq!(Timing::Point.discount_exponent(4), 4.0);
    assert_eq!(Timing::End.discount_exponent(4), 5.0);
    assert_eq!(Timing::Mid.discount_exponent(4), 4.5);
}

#[test]
fn expr_path_locates_the_node_that_produced_each_edge() {
    // (reserve[t-1] + premium_income[t-1]) * (1 + valuation_rate) — the §3.0.1 worked example.
    let expr = mul(
        add(
            Expr::Lag {
                name: "reserve".into(),
                k: 1,
            },
            Expr::Lag {
                name: "premium_income".into(),
                k: 1,
            },
        ),
        add(Expr::f64(1.0), Expr::r#ref("valuation_rate")),
    );

    let paths: Vec<(String, String)> = expr
        .reference_paths(PathRoot::Expr)
        .into_iter()
        .map(|(p, e)| {
            let name = match e {
                Expr::Ref { name } | Expr::Lag { name, .. } | Expr::At { name, .. } => name.clone(),
                Expr::Lookup { table, .. } => table.clone(),
                _ => unreachable!(),
            };
            (name, p.to_string())
        })
        .collect();

    assert_eq!(
        paths,
        vec![
            ("reserve".to_string(), "expr.lhs.lhs".to_string()),
            ("premium_income".to_string(), "expr.lhs.rhs".to_string()),
            ("valuation_rate".to_string(), "expr.rhs.rhs".to_string()),
        ]
    );

    // Paths resolve back to the node, and round-trip through their text form.
    for (_, text) in &paths {
        let path: ExprPath = text.parse().unwrap();
        assert_eq!(path.to_string(), *text);
        assert!(expr.resolve(&path).is_some());
    }

    // The empty path is legal and means the whole expression.
    let root = ExprPath::root(PathRoot::Expr);
    assert_eq!(root.to_string(), "expr");
    assert_eq!(expr.resolve(&root), Some(&expr));

    // A path that does not exist resolves to None — how a stale cached layout is detected.
    let stale: ExprPath = "expr.lhs.lhs.rhs".parse().unwrap();
    assert_eq!(expr.resolve(&stale), None);
}

#[test]
fn expr_path_covers_every_segment_of_the_grammar() {
    let expr = Expr::If {
        cond: Box::new(Expr::Unary {
            op: UnaryOp::Not,
            operand: Box::new(Expr::r#ref("flag")),
        }),
        then: Box::new(Expr::Call {
            func: "min".into(),
            args: vec![
                Expr::r#ref("a"),
                Expr::Lookup {
                    table: "tbl".into(),
                    keys: vec![Expr::r#ref("k0"), Expr::r#ref("k1")],
                },
            ],
        }),
        otherwise: Box::new(Expr::Agg {
            op: AggOp::Sum,
            value: Box::new(Expr::r#ref("x")),
            pred: Some(Box::new(Expr::r#ref("cond"))),
        }),
    };

    let seen: Vec<String> = expr
        .reference_paths(PathRoot::Expr)
        .iter()
        .map(|(p, _)| p.to_string())
        .collect();
    assert_eq!(
        seen,
        vec![
            "expr.cond.operand",
            "expr.then.arg0",
            "expr.then.arg1",
            "expr.then.arg1.key0",
            "expr.then.arg1.key1",
            "expr.else.value",
            "expr.else.pred",
        ]
    );
    for text in &seen {
        let path: ExprPath = text.parse().unwrap();
        assert!(expr.resolve(&path).is_some(), "{text} did not resolve");
    }
}

#[test]
fn expr_paths_into_init_are_rooted_at_init() {
    let reserve = model_module().component("reserve").unwrap().clone();
    let init_paths: Vec<String> = reserve
        .reference_paths()
        .iter()
        .map(|(p, _)| p.to_string())
        .filter(|p| p.starts_with("init"))
        .collect();
    assert_eq!(init_paths, vec!["init"]);

    let path: ExprPath = "init".parse().unwrap();
    assert_eq!(path.root_kind(), PathRoot::Init);
    assert_eq!(reserve.resolve(&path), Some(&Expr::r#ref("bel")));
}

#[test]
fn malformed_expr_paths_are_rejected() {
    assert!("body.lhs".parse::<ExprPath>().is_err());
    assert!("expr.left".parse::<ExprPath>().is_err());
    assert!("expr.arg".parse::<ExprPath>().is_err());
    assert!("expr.keyX".parse::<ExprPath>().is_err());
    assert!("expr.arg12".parse::<ExprPath>().is_ok());
}

#[test]
fn qualified_id_uses_a_dot_and_nothing_else() {
    let c = model_module().component("bel").unwrap().clone();
    assert_eq!(
        c.qualified_id("term_assurance.reserves"),
        "term_assurance.reserves.bel"
    );
}

#[test]
fn outputs_are_a_property_of_the_model_and_match_the_product_manifest() {
    let m = model_module();
    let mut model_outputs: Vec<&str> = m.outputs().iter().map(|c| c.name.as_str()).collect();
    model_outputs.sort_unstable();

    let mut declared = product_file().product.outputs;
    declared.sort();

    assert_eq!(model_outputs, declared, "E0107 would fire on a mismatch");
    assert!(Kind::Output.is_emitted());
    assert!(!Kind::Derived.is_emitted());
    assert!(Kind::InputModelpoint.is_input());
    assert!(!Kind::Derived.is_input());
}

#[test]
fn inputs_have_no_expr_and_derived_components_always_do() {
    let m = model_module();
    for c in &m.components {
        assert!(!c.kind.is_input());
        assert!(c.expr.is_some(), "{} has no expr", c.name);
    }
    // There is no Kind::Abstract to construct (Q10) — the enum has six variants and none of
    // them is a hole.
    let json = serde_json::to_string(&Kind::Derived).unwrap();
    assert!(serde_json::from_str::<Kind>("\"Abstract\"").is_err());
    assert_eq!(json, "\"Derived\"");
}

#[test]
fn on_missing_policies_parse_and_render_exactly() {
    let cases = [
        ("error", OnMissing::Error),
        ("default(0)", OnMissing::Default(LitValue::Int(0))),
        ("default(0.05)", OnMissing::Default(LitValue::Float(0.05))),
        ("default(true)", OnMissing::Default(LitValue::Bool(true))),
        (
            "interpolate(age)",
            OnMissing::Interpolate("age".to_string()),
        ),
    ];
    for (text, policy) in cases {
        assert_eq!(text.parse::<OnMissing>().unwrap(), policy);
        assert_eq!(policy.to_string(), text);
        assert_eq!(
            serde_json::to_string(&policy).unwrap(),
            format!("\"{text}\"")
        );
    }
    // There is no "silently produce NaN" setting.
    assert!("nan".parse::<OnMissing>().is_err());
    assert!("null".parse::<OnMissing>().is_err());
    assert!("interpolate()".parse::<OnMissing>().is_err());
    assert_eq!(OnMissing::default(), OnMissing::Error);
}

#[test]
fn table_source_selects_the_resolver() {
    assert_eq!(
        "tables/sa8990.csv".parse::<TableSource>().unwrap(),
        TableSource::File("tables/sa8990.csv".into())
    );
    assert_eq!(
        "resource:sa8990".parse::<TableSource>().unwrap(),
        TableSource::Resource("sa8990".into())
    );
    assert_eq!(
        "inline".parse::<TableSource>().unwrap(),
        TableSource::Inline
    );
    assert_eq!(
        TableSource::Resource("sa8990".into()).to_string(),
        "resource:sa8990"
    );
}

#[test]
fn presence_bits_exist_only_where_2_11_says_they_do() {
    let schema = schema_module();

    // A table with `on_missing = default(...)` carries a hit flag; `error` does not.
    assert!(!schema.table("sa8990").unwrap().has_presence_bit());
    assert!(schema.table("lapse_rates").unwrap().has_presence_bit());

    // An optional modelpoint field carries a presence lane; a required one cannot be missing,
    // so `is_null` on it is E0602.
    let optional = schema
        .modelpoint_fields
        .iter()
        .find(|f| f.name == "commission_rate")
        .unwrap();
    assert!(optional.has_presence_bit());
    assert!(
        optional.default_value.is_some(),
        "optional fields must default"
    );

    let required = schema
        .modelpoint_fields
        .iter()
        .find(|f| f.name == "sum_assured")
        .unwrap();
    assert!(!required.has_presence_bit());
}

#[test]
fn table_arity_is_the_key_count_a_lookup_must_supply() {
    let schema = schema_module();
    assert_eq!(schema.table("sa8990").unwrap().arity(), 3);
    assert_eq!(schema.table("lapse_rates").unwrap().arity(), 1);

    let lookup = model_module().component("qx").unwrap().clone();
    let keys = match lookup.expr.as_ref().unwrap() {
        Expr::Binary { lhs, .. } => match lhs.as_ref() {
            Expr::Lookup { keys, .. } => keys.len(),
            other => panic!("expected a lookup, got {other:?}"),
        },
        other => panic!("expected a binary node, got {other:?}"),
    };
    assert_eq!(keys, schema.table("sa8990").unwrap().arity());
}

#[test]
fn inline_tables_carry_their_rows_in_keys_then_values_order() {
    let t = schema_module().table("lapse_rates").unwrap().clone();
    assert_eq!(t.source, TableSource::Inline);
    let rows = t.rows.unwrap();
    assert!(rows
        .iter()
        .all(|r| r.len() == t.keys.len() + t.values.len()));
    // Sorted ascending by the key tuple.
    let keys: Vec<&LitValue> = rows.iter().map(|r| &r[0]).collect();
    assert_eq!(
        keys,
        vec![&LitValue::Int(1), &LitValue::Int(2), &LitValue::Int(3)]
    );
}

#[test]
fn enums_are_unordered_but_their_declaration_fixes_the_dictionary_code() {
    let e = schema_module().enums[0].clone();
    assert_eq!(e.code_of("M"), Some(0));
    assert_eq!(e.code_of("F"), Some(1));
    assert_eq!(e.code_of("X"), None);
}

#[test]
fn the_timeline_owns_t_and_generates_its_own_inputs() {
    let tl = schema_module().timeline.unwrap();
    assert_eq!(tl.periods, 40);
    assert_eq!(tl.basis.periods_per_year(), 1);
    assert_eq!(Basis::Monthly.periods_per_year(), 12);
    assert_eq!(Basis::Quarterly.periods_per_year(), 4);

    for name in ["t", "policy_year", "is_anniversary", "year_frac"] {
        assert!(is_timeline_field(name), "{name} should be a timeline input");
    }
    assert!(!is_timeline_field("reserve"));

    // There is no field on RunConfig through which a run could set the timeline (E0101).
    let run = serde_json::to_value(run_file().run).unwrap();
    for forbidden in [
        "periods",
        "basis",
        "origin",
        "valuation_date",
        "year_convention",
    ] {
        assert!(
            run.get(forbidden).is_none(),
            "a run must not be able to set {forbidden}"
        );
    }
}

#[test]
fn run_digest_payload_excludes_exactly_out_and_exec() {
    let mut a = run_file();
    let base = a.digest_payload();

    // Changing execution settings or the output directory does not change the payload.
    a.run.exec.threads = Some(64);
    a.run.exec.chunk_size = 1;
    a.run.exec.progress = false;
    a.run.out = "runs/somewhere-else".into();
    assert_eq!(a.digest_payload(), base);

    // Everything that can change a number does change it.
    let mut b = run_file();
    b.run.emit = Emit::All;
    assert_ne!(b.digest_payload(), base);

    let mut c = run_file();
    c.run.retain = Retain::Full;
    assert_ne!(c.digest_payload(), base);

    let mut d = run_file();
    d.run.max_errors = 5;
    assert_ne!(d.digest_payload(), base);

    let mut e = run_file();
    e.run.on_trap = OnTrap::Continue;
    assert_ne!(e.digest_payload(), base);

    let mut f = run_file();
    f.run.sum_kahan = true;
    assert_ne!(f.digest_payload(), base);

    let mut g = run_file();
    g.run
        .tables
        .insert("lapse_rates".into(), "resource:l2".into());
    assert_ne!(g.digest_payload(), base);

    let mut h = run_file();
    h.aggregations[0].op = AggregationOp::Mean;
    assert_ne!(h.digest_payload(), base);

    let mut i = run_file();
    i.solves[0].tolerance = 1e-4;
    assert_ne!(i.digest_payload(), base);

    // And the payload itself names the exclusion rule in one place.
    let payload = base.get("run").unwrap();
    for key in RUN_DIGEST_EXCLUDED {
        assert!(payload.get(*key).is_none(), "{key} must be excluded");
    }
    assert!(payload.get("storage_precision").is_some());
    assert!(payload.get("allow_table_drift").is_some());
}

#[test]
fn f32_storage_is_representable_but_not_supported_in_1_0() {
    assert!(StoragePrecision::F64.is_supported_in_1_0());
    assert!(!StoragePrecision::F32.is_supported_in_1_0(), "E0108");
    // The key exists in the grammar so 1.1 can enable it as a minor bump.
    assert_eq!(
        serde_json::from_str::<StoragePrecision>("\"f32\"").unwrap(),
        StoragePrecision::F32
    );
}

#[test]
fn solve_defaults_follow_scope() {
    let mut s = run_file().solves.pop().unwrap();
    assert_eq!(s.effective_on_not_converged(), OnNotConverged::Warn);
    assert!(s.writes_per_mp_file());

    s.scope = SolveScope::Portfolio;
    assert_eq!(s.effective_on_not_converged(), OnNotConverged::Error);
    assert!(
        !s.writes_per_mp_file(),
        "no side file for a portfolio solve"
    );

    s.on_not_converged = Some(OnNotConverged::Warn);
    assert_eq!(s.effective_on_not_converged(), OnNotConverged::Warn);
}

#[test]
fn aggregation_group_key_is_the_normative_join_key() {
    let agg = Aggregation {
        name: "bel_by_cohort".into(),
        group_by: vec!["product_code".into(), "entry_year_band".into()],
        measure: "bel".into(),
        op: AggregationOp::WeightedMean,
        weight: Some("num_pols_if".into()),
        filter: Some("in_force_at_val".into()),
        over_t: Some(OverT::Total),
    };
    assert_eq!(
        agg.group_key(&["TERM_UK".into(), "2015-2019".into()]),
        "product_code=TERM_UK|entry_year_band=2015-2019"
    );
    assert!(agg.op.requires_weight());
    assert!(!AggregationOp::Sum.requires_weight());
}

#[test]
fn the_builtin_set_is_closed_and_sorted_within_its_groups() {
    for name in [
        "npv",
        "round",
        "to_monthly",
        "count_while",
        "retime",
        "coalesce",
    ] {
        assert!(is_builtin(name), "{name} should be a builtin");
    }
    for name in ["lambda", "sumproduct", "eval", "map", "if"] {
        assert!(!is_builtin(name), "{name} must not be a builtin");
    }
    for name in TIMING_OPS {
        assert!(is_builtin(name));
        assert!(is_timing_op(name));
    }
    assert!(!is_timing_op("npv"));
    // No duplicates.
    let mut sorted = BUILTINS.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), BUILTINS.len());
}

#[test]
fn dtypes_without_a_zero_cannot_default_a_pre_origin_lag() {
    assert!(DType::F64.has_zero());
    assert!(DType::I64.has_zero());
    assert!(DType::Bool.has_zero());
    // date/str/enum have no zero: a Lag on them without an `init` is a definition-time error.
    assert!(!DType::Date.has_zero());
    assert!(!DType::Str.has_zero());
    assert!(!DType::Enum("Gender".into()).has_zero());
}

#[test]
fn literal_values_are_checked_against_their_dtype() {
    assert!(LitValue::Float(1.0).fits(&DType::F64));
    assert!(LitValue::Int(1).fits(&DType::F64), "int literals widen");
    assert!(LitValue::Int(1).fits(&DType::I64));
    assert!(!LitValue::Float(1.5).fits(&DType::I64));
    assert!(LitValue::Bool(true).fits(&DType::Bool));
    assert!(!LitValue::Bool(true).fits(&DType::F64));
    assert!(LitValue::Text("2026-06-30".into()).fits(&DType::Date));
    assert!(LitValue::Text("F".into()).fits(&DType::Enum("Gender".into())));
    assert!(!LitValue::Text("F".into()).fits(&DType::F64));
}

#[test]
fn expr_size_is_bounded_and_walkable() {
    let m = model_module();
    let num_pols = m.component("num_pols_if").unwrap();
    let expr = num_pols.expr.as_ref().unwrap();
    assert_eq!(expr.size(), 10);
    assert!(!expr.contains_agg());
    assert!(m
        .component("pv_claims")
        .unwrap()
        .expr
        .as_ref()
        .unwrap()
        .contains_agg());
}

#[test]
fn metadata_survives_the_encoding_and_never_affects_evaluation() {
    let reserve = model_module().component("reserve").unwrap().clone();
    let text = serde_json::to_string(&reserve).unwrap();
    let back: Component = serde_json::from_str(&text).unwrap();
    assert_eq!(back, reserve);
    assert_eq!(back.meta.id.as_deref(), Some("01J8Q2K7"));
    assert_eq!(back.meta.authored_by, Some(AuthoredBy::Migration));
    assert_eq!(
        back.meta.source.as_ref().unwrap().variable.as_deref(),
        Some("BEL_TOT")
    );
    assert_eq!(back.meta.origin_span.as_ref().unwrap().line, 34);

    // An empty meta block is not written at all, so it never pollutes a diff.
    let plain = per_mp_output("pv", Unit::Money, Expr::r#ref("x"));
    let v: Value = serde_json::to_value(&plain).unwrap();
    assert!(v.get("meta").is_none());
}
