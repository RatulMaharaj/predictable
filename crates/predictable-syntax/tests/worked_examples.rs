//! The worked example of `01-ir.md` §6 must parse — and it is read out of the
//! spec itself, so the parser and the normative document cannot drift apart.

use std::path::PathBuf;

use predictable_syntax::{
    ast::{DType, Kind, Origin, Shape, TimelineBasis, Timing, Unit},
    expr::{BinOp, Expr, Lag, Lit},
    parse, DocumentKind, OnMissing, SourceMap,
};

fn spec_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/design/01-ir.md")
}

/// Every ```toml fence inside `## 6. Worked example`, in document order.
fn worked_example_blocks() -> Vec<String> {
    let text = std::fs::read_to_string(spec_path()).expect("01-ir.md is readable");
    let start = text.find("## 6. Worked example").expect("§6 exists");
    let rest = &text[start..];
    let end = rest.find("\n## 7.").expect("§7 follows §6");
    let section = &rest[..end];

    let mut blocks = Vec::new();
    let mut in_block = false;
    let mut cur = String::new();
    for line in section.lines() {
        if line.trim_start().starts_with("```") {
            if in_block {
                blocks.push(std::mem::take(&mut cur));
                in_block = false;
            } else {
                in_block = line.trim() == "```toml";
            }
            continue;
        }
        if in_block {
            cur.push_str(line);
            cur.push('\n');
        }
    }
    blocks
}

#[test]
fn all_five_worked_example_files_parse_without_diagnostics() {
    let blocks = worked_example_blocks();
    assert_eq!(
        blocks.len(),
        5,
        "§6 has schema, model, assumptions, product and run"
    );

    let names = [
        "schema.pir",
        "model.pir",
        "base.pir",
        "term_uk.pir",
        "run.pir",
    ];
    let mut sources = SourceMap::new();
    for (name, text) in names.iter().zip(&blocks) {
        let parsed = parse(&mut sources, *name, text.clone());
        let rendered: Vec<String> = parsed
            .diagnostics
            .iter()
            .map(|d| {
                let loc = d.primary_span().map(|s| sources.location(s));
                format!("{} {:?} {}", d.code, loc, d.message)
            })
            .collect();
        assert!(
            rendered.is_empty(),
            "{name} produced diagnostics: {rendered:#?}"
        );
    }
}

fn parse_block(i: usize, name: &str) -> (SourceMap, predictable_syntax::Parsed) {
    let text = worked_example_blocks().remove(i);
    let mut sources = SourceMap::new();
    let parsed = parse(&mut sources, name.to_string(), text);
    assert!(
        !parsed.has_errors(),
        "{name}: {:?}",
        parsed.diagnostics.codes()
    );
    (sources, parsed)
}

#[test]
fn schema_module_lowers_to_the_declared_data_model() {
    let (_sources, parsed) = parse_block(0, "schema.pir");
    let doc = &parsed.document;

    assert_eq!(doc.format.as_deref(), Some("pir/1"));
    assert_eq!(doc.module.as_ref().unwrap().value, "term_assurance");
    assert_eq!(doc.kind(), DocumentKind::Module);

    let tl = doc.timeline.as_ref().expect("timeline");
    assert_eq!(tl.basis, TimelineBasis::Annual);
    assert_eq!(tl.periods, 40);
    assert_eq!(tl.origin, Origin::Policy);
    assert_eq!(tl.valuation_date.as_deref(), Some("2026-06-30"));
    assert_eq!(tl.year_convention.as_deref(), Some("act/365"));

    assert_eq!(doc.enums.len(), 1);
    assert_eq!(doc.enums[0].name.value, "Gender");
    assert_eq!(doc.enums[0].values, vec!["M", "F"]);

    // Seven modelpoint fields, all `required = true`, one of them the key.
    assert_eq!(doc.modelpoint_fields.len(), 7);
    assert!(doc.modelpoint_fields.iter().all(|f| f.required));
    let key: Vec<&str> = doc
        .modelpoint_fields
        .iter()
        .filter(|f| f.key)
        .map(|f| f.name.value.as_str())
        .collect();
    assert_eq!(key, vec!["policy_number"]);
    let gender = &doc.modelpoint_fields[2];
    assert_eq!(gender.name.value, "gender");
    assert_eq!(gender.dtype, DType::Enum("Gender".into()));
    let sum_assured = &doc.modelpoint_fields[4];
    assert_eq!(sum_assured.dtype, DType::F64);
    assert_eq!(sum_assured.unit, Unit::Money);

    // Assumptions keep their rate basis: this is what makes `/12` catchable.
    assert_eq!(doc.assumptions.len(), 5);
    let val_rate = &doc.assumptions[0];
    assert_eq!(val_rate.name.value, "valuation_rate");
    assert_eq!(val_rate.shape, Shape::Scalar);
    assert_eq!(
        val_rate.unit,
        Unit::Rate(predictable_syntax::ast::RateBasis::Annual)
    );

    // Tables, with ordered typed keys and per-key policies.
    assert_eq!(doc.tables.len(), 2);
    let sa = doc.table("sa8990").expect("sa8990");
    assert_eq!(sa.keys.len(), 3);
    assert_eq!(sa.keys[0].name, "age");
    assert_eq!(sa.keys[0].policy, predictable_syntax::ast::KeyPolicy::Clamp);
    assert_eq!(sa.keys[1].dtype, DType::Enum("Gender".into()));
    assert_eq!(sa.keys[2].dtype, DType::Bool);
    assert_eq!(sa.values.len(), 1);
    assert_eq!(sa.values[0].unit, Unit::Prob);
    assert_eq!(sa.on_missing, OnMissing::Error);
    assert_eq!(sa.source, "tables/sa8990.csv");
    assert!(sa.digest.as_ref().unwrap().starts_with("sha256:"));

    let lapses = doc.table("lapse_rates").expect("lapse_rates");
    assert_eq!(
        lapses.keys[0].policy,
        predictable_syntax::ast::KeyPolicy::Step
    );
    assert_eq!(lapses.on_missing, OnMissing::Default("0.0".into()));
}

#[test]
fn model_module_lowers_every_component() {
    let (_sources, parsed) = parse_block(1, "model.pir");
    let doc = &parsed.document;
    let arena = &doc.arena;

    assert_eq!(doc.imports, vec!["term_assurance/schema"]);
    assert_eq!(doc.components.len(), 17);

    // Declaration order is preserved — it is §3.2's tiebreak.
    let names: Vec<&str> = doc
        .components
        .iter()
        .map(|c| c.name.value.as_str())
        .collect();
    assert_eq!(names[0], "age");
    assert_eq!(names[names.len() - 1], "reserve");

    // Outputs are exactly the product manifest of §6, in declaration order.
    let outputs: Vec<&str> = doc.outputs().map(|c| c.name.value.as_str()).collect();
    assert_eq!(
        outputs,
        vec![
            "net_cashflow",
            "pv_premiums",
            "pv_claims",
            "pv_expenses",
            "bel",
            "reserve"
        ]
    );

    // A plain binary formula.
    let deaths = doc.component("deaths").unwrap();
    assert_eq!(deaths.kind, Kind::Derived);
    assert_eq!(deaths.timing, Some(Timing::End));
    match arena.get(deaths.expr.unwrap()) {
        Expr::Binary {
            op: BinOp::Mul,
            lhs,
            rhs,
        } => {
            assert_eq!(arena.get(*lhs), &Expr::Ref("num_pols_if".into()));
            assert_eq!(arena.get(*rhs), &Expr::Ref("qx".into()));
        }
        other => panic!("unexpected: {other:?}"),
    }

    // A lookup with three keys.
    let qx = doc.component("qx").unwrap();
    match arena.get(qx.expr.unwrap()) {
        Expr::Binary {
            op: BinOp::Mul,
            lhs,
            ..
        } => match arena.get(*lhs) {
            Expr::Lookup { table, keys } => {
                assert_eq!(table, "sa8990");
                assert_eq!(keys.len(), 3);
                assert_eq!(arena.get(keys[1]), &Expr::Ref("gender".into()));
            }
            other => panic!("unexpected: {other:?}"),
        },
        other => panic!("unexpected: {other:?}"),
    }

    // Self-reference with lag 1 plus an `init` — legal per §3.1.
    let n = doc.component("num_pols_if").unwrap();
    assert_eq!(arena.get(n.init.unwrap()), &Expr::Lit(Lit::Float(1.0)));
    let refs = arena.references(n.expr.unwrap(), "expr");
    let self_ref = refs.iter().find(|r| r.name == "num_pols_if").unwrap();
    assert_eq!(self_ref.lag, Lag::Back(1));
    // `if in_term then 1.0 else 0.0` survives as a value conditional.
    assert!(refs
        .iter()
        .any(|r| r.name == "in_term" && r.path.contains("cond")));

    // `init` may hold a bare reference to a stage-2 value (§8.2).
    let reserve = doc.component("reserve").unwrap();
    assert_eq!(arena.get(reserve.init.unwrap()), &Expr::Ref("bel".into()));
    assert_eq!(
        reserve.stage(arena),
        1,
        "reserve's own expr has no aggregate"
    );

    // `npv` makes a component stage-2 and PerMP components carry no timing.
    let pv_premiums = doc.component("pv_premiums").unwrap();
    assert_eq!(pv_premiums.shape, Shape::PerMP);
    assert_eq!(pv_premiums.timing, None);
    assert_eq!(pv_premiums.stage(arena), 2);
    assert_eq!(doc.component("bel").unwrap().stage(arena), 1);

    // Nested calls: `retime(premium_income, mid) - ...`
    let ncf = doc.component("net_cashflow").unwrap();
    let ncf_refs = arena.references(ncf.expr.unwrap(), "expr");
    let names: Vec<&str> = ncf_refs.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["premium_income", "death_claims", "renewal_expenses"]
    );
    assert_eq!(ncf_refs[0].path, "expr.lhs.lhs.arg0");
}

#[test]
fn expr_paths_from_the_spec_resolve_to_the_right_nodes() {
    // §3.0.1's worked path example, taken from `reserve`.
    let (_sources, parsed) = parse_block(1, "model.pir");
    let doc = &parsed.document;
    let arena = &doc.arena;
    let reserve = doc.component("reserve").unwrap().expr.unwrap();

    let lhs = arena
        .resolve_path(reserve, "expr", "expr.lhs.lhs.lhs.lhs")
        .unwrap();
    assert_eq!(
        arena.get(lhs),
        &Expr::Lag {
            name: "reserve".into(),
            k: 1
        }
    );
    let rate = arena.resolve_path(reserve, "expr", "expr.rhs.rhs").unwrap();
    assert_eq!(arena.get(rate), &Expr::Ref("valuation_rate".into()));

    // The empty path is the whole expression; a stale path resolves to None.
    let bel_init = doc.component("reserve").unwrap().init.unwrap();
    assert_eq!(
        arena.resolve_path(bel_init, "init", "init").unwrap(),
        bel_init
    );
    assert!(arena
        .resolve_path(reserve, "expr", "expr.lhs.arg7")
        .is_none());
    assert!(arena.resolve_path(bel_init, "init", "init.lhs").is_none());
}

#[test]
fn assumption_set_product_and_run_files_lower() {
    let (_s, assumptions) = parse_block(2, "base.pir");
    let doc = &assumptions.document;
    assert_eq!(doc.kind(), DocumentKind::AssumptionSet);
    assert_eq!(doc.assumption_set.as_deref(), Some("base"));
    assert_eq!(doc.model_module.as_deref(), Some("term_assurance"));
    let values: Vec<(&str, &predictable_syntax::Value)> = doc
        .assumption_values
        .iter()
        .map(|(k, v)| (k.value.as_str(), v))
        .collect();
    assert_eq!(values.len(), 5);
    assert_eq!(values[0].0, "valuation_rate");
    assert_eq!(values[0].1, &predictable_syntax::Value::Float(0.035));
    assert_eq!(values[3].1, &predictable_syntax::Value::Float(45.0));

    let (_s, product) = parse_block(3, "term_uk.pir");
    let p = product.document.product.as_ref().expect("product");
    assert_eq!(product.document.kind(), DocumentKind::Product);
    assert_eq!(p.name, "TERM_UK");
    assert_eq!(
        p.modules,
        vec!["term_assurance/schema", "term_assurance/model"]
    );
    assert_eq!(p.outputs.len(), 6);
    assert_eq!(p.key_field.as_deref(), Some("policy_number"));

    let (_s, run) = parse_block(4, "run.pir");
    let doc = &run.document;
    assert_eq!(doc.kind(), DocumentKind::Run);
    let r = doc.run.as_ref().expect("run");
    assert_eq!(r.product.as_deref(), Some("products/term_uk"));
    assert_eq!(r.emit.as_deref(), Some("outputs"));
    assert_eq!(r.retain.as_deref(), Some("ring"));
    assert_eq!(r.on_trap.as_deref(), Some("abort"));
    assert_eq!(r.exec.threads, Some(8));
    assert_eq!(r.exec.chunk_size, Some(1024));
    assert_eq!(doc.aggregations.len(), 1);
    assert_eq!(doc.aggregations[0].name, "bel_by_gender");
    assert_eq!(doc.aggregations[0].group_by, vec!["gender"]);
    assert_eq!(doc.aggregations[0].op, "sum");
}

#[test]
fn the_full_run_block_of_section_8_4_2_parses() {
    // The §8.4.2 example exercises `[run.tables]`, `[[solve]]` and every key.
    let text = r#"
format = "pir/1"

[run]
product      = "products/term_uk"
assumptions  = "assumptions/base"
modelpoints  = "data/term.mpf.parquet"
out          = "runs/2026-06-30-base"

emit         = "outputs"
emit_list    = []
retain       = "ring"
storage_precision = "f64"

on_trap      = "abort"
max_errors   = 100
allow_table_drift = false
sum_kahan    = false

[run.tables]
sa8990 = "resource:sa8990_2026"

[run.exec]
threads    = 8
chunk_size = 1024
progress   = true

[[solve]]
name      = "premium_solve"
target    = "bel"
to        = 0.0
vary      = "annual_premium"
scope     = "per_mp"
tolerance = 1e-8
max_iter  = 50
method    = "brent"
bracket   = [0.0, 1.0e6]

[[aggregation]]
name     = "bel_by_cohort"
group_by = ["product_code"]
measure  = "bel"
op       = "sum"
"#;
    let mut sources = SourceMap::new();
    let parsed = parse(&mut sources, "run.pir", text);
    assert!(
        parsed.diagnostics.is_empty(),
        "{:?}",
        parsed.diagnostics.codes()
    );
    let run = parsed.document.run.as_ref().unwrap();
    assert_eq!(run.max_errors, Some(100));
    assert_eq!(run.allow_table_drift, Some(false));
    assert_eq!(
        run.tables,
        vec![("sa8990".to_string(), "resource:sa8990_2026".to_string())]
    );
    assert_eq!(run.exec.progress, Some(true));
    let solve = &parsed.document.solves[0];
    assert_eq!(solve.name, "premium_solve");
    assert_eq!(solve.tolerance, Some(1e-8));
    assert_eq!(solve.bracket, Some((0.0, 1.0e6)));
    assert_eq!(solve.method.as_deref(), Some("brent"));
}

#[test]
fn an_inline_table_declaration_carries_its_rows() {
    let text = r#"
format = "pir/1"
module = "m"

[[table]]
name   = "lapse_rates"
keys   = [{ name = "policy_year", dtype = "i64", policy = "step" }]
values = [{ name = "lapse_pa", dtype = "f64", unit = "prob" }]
source = "inline"
digest = "sha256:31aa"
rows   = [
  [1, 0.12],
  [2, 0.08],
  [3, 0.05],
]
"#;
    let mut sources = SourceMap::new();
    let parsed = parse(&mut sources, "schema.pir", text);
    assert!(
        parsed.diagnostics.is_empty(),
        "{:?}",
        parsed.diagnostics.codes()
    );
    let t = parsed.document.table("lapse_rates").unwrap();
    assert_eq!(t.rows.len(), 3);
    assert_eq!(
        t.rows[1],
        vec![
            predictable_syntax::Value::Int(2),
            predictable_syntax::Value::Float(0.08)
        ]
    );
}
