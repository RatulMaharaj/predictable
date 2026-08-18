//! A shared fixture: the term-assurance model of `01-ir.md` §6, built as IR values.

#![allow(dead_code)]

use predictable_ir::*;

pub fn series(name: &str, dtype: DType, unit: Unit, timing: Timing, expr: Expr) -> Component {
    let mut c = Component::derived(name, dtype, Shape::Series, expr);
    c.unit = unit;
    c.timing = Some(timing);
    c
}

pub fn per_mp_output(name: &str, unit: Unit, expr: Expr) -> Component {
    let mut c = Component::derived(name, DType::F64, Shape::PerMp, expr);
    c.kind = Kind::Output;
    c.unit = unit;
    c
}

pub fn mul(lhs: Expr, rhs: Expr) -> Expr {
    Expr::binary(BinaryOp::Mul, lhs, rhs)
}

pub fn add(lhs: Expr, rhs: Expr) -> Expr {
    Expr::binary(BinaryOp::Add, lhs, rhs)
}

pub fn sub(lhs: Expr, rhs: Expr) -> Expr {
    Expr::binary(BinaryOp::Sub, lhs, rhs)
}

/// `npv(x, disc)` — an `Agg` with the discount series as its predicate slot is *not* how npv is
/// modelled; npv is a `Call` in the grammar and an `Agg` node in the tree, with the discount
/// series carried as the aggregate's second operand. Here it is the `pred` slot, which §2.6
/// reserves for the aggregate's optional second expression.
pub fn npv(x: Expr, disc: Expr) -> Expr {
    Expr::Agg {
        op: AggOp::Npv,
        value: Box::new(x),
        pred: Some(Box::new(disc)),
    }
}

/// The schema module: timeline, enum, modelpoint schema, assumptions, tables.
pub fn schema_module() -> Module {
    let mut m = Module::new("term_assurance");
    m.timeline = Some(Timeline {
        basis: Basis::Annual,
        periods: 40,
        origin: Origin::Policy,
        valuation_date: "2026-06-30".into(),
        year_convention: "act/365".into(),
    });
    m.enums.push(EnumDecl {
        name: "Gender".into(),
        values: vec!["M".into(), "F".into()],
    });

    let field = |name: &str, dtype: DType, unit: Unit, required: bool, key: bool| ModelpointField {
        name: name.into(),
        dtype,
        unit,
        required,
        key,
        default_value: None,
        doc: None,
    };
    m.modelpoint_fields = vec![
        field("policy_number", DType::Str, Unit::None, true, true),
        field("entry_age", DType::I64, Unit::Years, true, false),
        field(
            "gender",
            DType::Enum("Gender".into()),
            Unit::None,
            true,
            false,
        ),
        field("smoker", DType::Bool, Unit::None, true, false),
        field("sum_assured", DType::F64, Unit::Money, true, false),
        field("annual_premium", DType::F64, Unit::Money, true, false),
        field("policy_term", DType::I64, Unit::Years, true, false),
        ModelpointField {
            name: "commission_rate".into(),
            dtype: DType::F64,
            unit: Unit::Factor,
            required: false,
            key: false,
            default_value: Some(LitValue::Float(0.0)),
            doc: Some("Optional; missingness is eliminated at load (IR 2.11).".into()),
        },
    ];

    let assumption = |name: &str, unit: Unit| AssumptionDecl {
        name: name.into(),
        dtype: DType::F64,
        unit,
        shape: Shape::Scalar,
        doc: None,
    };
    m.assumptions = vec![
        assumption("valuation_rate", Unit::Rate(RateBasis::Annual)),
        assumption("premium_escalation", Unit::Rate(RateBasis::Annual)),
        assumption("expense_inflation", Unit::Rate(RateBasis::Annual)),
        assumption("renewal_expense_pa", Unit::Money),
        assumption("mortality_loading", Unit::Factor),
    ];

    m.tables = vec![
        TableDecl {
            name: "sa8990".into(),
            keys: vec![
                TableKey {
                    name: "age".into(),
                    dtype: DType::I64,
                    policy: KeyPolicy::Clamp,
                },
                TableKey {
                    name: "gender".into(),
                    dtype: DType::Enum("Gender".into()),
                    policy: KeyPolicy::Exact,
                },
                TableKey {
                    name: "smoker".into(),
                    dtype: DType::Bool,
                    policy: KeyPolicy::Exact,
                },
            ],
            values: vec![TableValue {
                name: "qx".into(),
                dtype: DType::F64,
                unit: Unit::Prob,
            }],
            on_missing: OnMissing::Error,
            source: TableSource::File("tables/sa8990.csv".into()),
            digest: Some("sha256:9f2c4b1a".into()),
            rows: None,
            doc: None,
        },
        TableDecl {
            name: "lapse_rates".into(),
            keys: vec![TableKey {
                name: "policy_year".into(),
                dtype: DType::I64,
                policy: KeyPolicy::Step,
            }],
            values: vec![TableValue {
                name: "lapse_pa".into(),
                dtype: DType::F64,
                unit: Unit::Prob,
            }],
            on_missing: OnMissing::Default(LitValue::Float(0.0)),
            source: TableSource::Inline,
            digest: Some("sha256:31aa7e0c".into()),
            rows: Some(vec![
                vec![LitValue::Int(1), LitValue::Float(0.12)],
                vec![LitValue::Int(2), LitValue::Float(0.08)],
                vec![LitValue::Int(3), LitValue::Float(0.05)],
            ]),
            doc: None,
        },
    ];
    m
}

/// The model module: components, including a lag self-reference, a lookup, an `If`, a stage-2
/// npv and an `init` that reads a stage-2 value.
pub fn model_module() -> Module {
    let mut m = Module::new("term_assurance");
    m.imports = vec!["term_assurance/schema".into()];

    m.components.push(series(
        "age",
        DType::I64,
        Unit::Years,
        Timing::Start,
        add(Expr::r#ref("entry_age"), Expr::r#ref("t")),
    ));

    m.components.push(series(
        "in_term",
        DType::Bool,
        Unit::None,
        Timing::Start,
        Expr::binary(BinaryOp::Lt, Expr::r#ref("t"), Expr::r#ref("policy_term")),
    ));

    m.components.push(series(
        "qx",
        DType::F64,
        Unit::Prob,
        Timing::End,
        mul(
            Expr::Lookup {
                table: "sa8990".into(),
                keys: vec![
                    Expr::r#ref("age"),
                    Expr::r#ref("gender"),
                    Expr::r#ref("smoker"),
                ],
            },
            Expr::r#ref("mortality_loading"),
        ),
    ));

    let mut num_pols = series(
        "num_pols_if",
        DType::F64,
        Unit::Count,
        Timing::Start,
        mul(
            mul(
                Expr::Lag {
                    name: "num_pols_if".into(),
                    k: 1,
                },
                sub(
                    Expr::f64(1.0),
                    Expr::Lag {
                        name: "qx".into(),
                        k: 1,
                    },
                ),
            ),
            Expr::If {
                cond: Box::new(Expr::r#ref("in_term")),
                then: Box::new(Expr::f64(1.0)),
                otherwise: Box::new(Expr::f64(0.0)),
            },
        ),
    );
    num_pols.init = Some(Expr::f64(1.0));
    num_pols.doc = Some("Survivorship. Self-referential with lag 1 — legal (IR 3.1).".into());
    m.components.push(num_pols);

    m.components.push(series(
        "premium_income",
        DType::F64,
        Unit::Money,
        Timing::Start,
        mul(Expr::r#ref("premium_rate"), Expr::r#ref("num_pols_if")),
    ));

    m.components.push(series(
        "death_claims",
        DType::F64,
        Unit::Money,
        Timing::End,
        mul(
            Expr::r#ref("sum_assured"),
            mul(Expr::r#ref("num_pols_if"), Expr::r#ref("qx")),
        ),
    ));

    let mut disc = series(
        "disc_factor",
        DType::F64,
        Unit::Factor,
        Timing::Point,
        Expr::binary(
            BinaryOp::Div,
            Expr::Lag {
                name: "disc_factor".into(),
                k: 1,
            },
            add(Expr::f64(1.0), Expr::r#ref("valuation_rate")),
        ),
    );
    disc.init = Some(Expr::f64(1.0));
    m.components.push(disc);

    m.components.push(per_mp_output(
        "pv_premiums",
        Unit::Money,
        npv(Expr::r#ref("premium_income"), Expr::r#ref("disc_factor")),
    ));
    m.components.push(per_mp_output(
        "pv_claims",
        Unit::Money,
        npv(Expr::r#ref("death_claims"), Expr::r#ref("disc_factor")),
    ));
    m.components.push(per_mp_output(
        "bel",
        Unit::Money,
        sub(Expr::r#ref("pv_claims"), Expr::r#ref("pv_premiums")),
    ));

    // `reserve` seeds itself from a stage-2 value through `init` — the single legal backward
    // channel (IR 8.2).
    let mut reserve = series(
        "reserve",
        DType::F64,
        Unit::Money,
        Timing::Point,
        mul(
            Expr::Lag {
                name: "reserve".into(),
                k: 1,
            },
            add(Expr::f64(1.0), Expr::r#ref("valuation_rate")),
        ),
    );
    reserve.kind = Kind::Output;
    reserve.init = Some(Expr::r#ref("bel"));
    reserve.meta = Meta {
        id: Some("01J8Q2K7".into()),
        authored_by: Some(AuthoredBy::Migration),
        source: Some(SourceRef {
            system: Some("prophet".into()),
            library: Some("TERM_UK".into()),
            variable: Some("BEL_TOT".into()),
            file: Some("TERM.VAR:118".into()),
        }),
        origin_span: Some(OriginSpan {
            file: "reserves.py".into(),
            line: 34,
            col_start: 12,
            col_end: 44,
        }),
        ..Meta::default()
    };
    m.components.push(reserve);
    m
}

pub fn product_file() -> ProductFile {
    ProductFile {
        format: predictable_ir::FORMAT.into(),
        product: Product {
            name: "TERM_UK".into(),
            modules: vec![
                "term_assurance/schema".into(),
                "term_assurance/model".into(),
            ],
            outputs: vec![
                "pv_premiums".into(),
                "pv_claims".into(),
                "bel".into(),
                "reserve".into(),
            ],
            key_field: "policy_number".into(),
            assumptions: Some("assumptions/base".into()),
            doc: Some("UK level-term assurance.".into()),
        },
    }
}

pub fn run_file() -> RunFile {
    let mut tables = std::collections::BTreeMap::new();
    tables.insert("sa8990".to_string(), "resource:sa8990_2026".to_string());
    RunFile {
        format: predictable_ir::FORMAT.into(),
        run: RunConfig {
            product: "products/term_uk".into(),
            assumptions: Some("assumptions/base".into()),
            modelpoints: "data/term.mpf.parquet".into(),
            out: "runs/2026-06-30-base".into(),
            emit: Emit::Outputs,
            emit_list: vec![],
            retain: Retain::Ring,
            storage_precision: StoragePrecision::F64,
            on_trap: OnTrap::Abort,
            max_errors: 100,
            allow_table_drift: false,
            sum_kahan: false,
            tables,
            exec: ExecConfig {
                threads: Some(8),
                chunk_size: 1024,
                progress: true,
            },
        },
        solves: vec![Solve {
            name: "premium_solve".into(),
            target: "bel".into(),
            to: 0.0,
            vary: "annual_premium".into(),
            scope: SolveScope::PerMp,
            tolerance: 1e-8,
            max_iter: 50,
            method: "brent".into(),
            bracket: Some([0.0, 1.0e6]),
            on_not_converged: None,
        }],
        aggregations: vec![Aggregation {
            name: "bel_by_gender".into(),
            group_by: vec!["gender".into()],
            measure: "bel".into(),
            op: AggregationOp::Sum,
            weight: None,
            filter: None,
            over_t: None,
        }],
    }
}
