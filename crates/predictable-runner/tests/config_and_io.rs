//! The `[run]` file → typed configuration lowering (`01-ir.md` §8.4.2), the modelpoint → chunk
//! pipeline over a real [`predictable_io`] source, and the portfolio solve.

mod common;

use common::{model, runner, TERM};
use predictable_io::{CsvSource, MpSchema};
use predictable_ir::run::{
    Aggregation, AggregationOp, Emit, OnTrap, OverT, Retain, Solve, SolveScope, StoragePrecision,
};
use predictable_runner::config::{assumption_values, run_file};
use predictable_runner::{load_chunks, minimal_run_file, CancelFlag, SerialExecutor};

fn parse(text: &str) -> predictable_syntax::ast::PirDocument {
    let mut sources = predictable_syntax::SourceMap::new();
    predictable_syntax::parse(&mut sources, "run.pir".to_string(), text.to_string()).document
}

const FULL_RUN: &str = r#"format = "pir/1"

[run]
product = "products/term_uk"
assumptions = "assumptions/base"
modelpoints = "data/term.mpf.parquet"
out = "runs/2026-06-30-base"
emit = "list"
emit_list = ["survivors"]
retain = "full"
storage_precision = "f64"
on_trap = "continue"
max_errors = 25
allow_table_drift = true
sum_kahan = false

[run.tables]
sa8990 = "resource:sa8990_2026"

[run.exec]
threads = 8
chunk_size = 512
progress = false

[[solve]]
name = "premium_solve"
target = "bel"
to = 0.0
vary = "annual_premium"
scope = "per_mp"
tolerance = 1e-9
max_iter = 40
method = "brent"
bracket = [0.0, 1000000.0]

[[aggregation]]
name = "bel_by_cohort"
group_by = ["product_code", "entry_year_band"]
measure = "bel"
op = "weighted_mean"
weight = "num_pols_if"
filter = "in_force_at_val"
over_t = "total"
"#;

#[test]
fn a_run_file_lowers_with_every_field_carried() {
    let run = run_file(&parse(FULL_RUN)).unwrap();
    assert_eq!(run.run.product, "products/term_uk");
    assert_eq!(run.run.emit, Emit::List);
    assert_eq!(run.run.emit_list, vec!["survivors".to_string()]);
    assert_eq!(run.run.retain, Retain::Full);
    assert_eq!(run.run.storage_precision, StoragePrecision::F64);
    assert_eq!(run.run.on_trap, OnTrap::Continue);
    assert_eq!(run.run.max_errors, 25);
    assert!(run.run.allow_table_drift);
    assert_eq!(run.run.tables["sa8990"], "resource:sa8990_2026");
    assert_eq!(run.run.exec.threads, Some(8));
    assert_eq!(run.run.exec.chunk_size, 512);
    assert!(!run.run.exec.progress);

    let solve: &Solve = &run.solves[0];
    assert_eq!(solve.scope, SolveScope::PerMp);
    assert_eq!(solve.tolerance, 1e-9);
    assert_eq!(solve.max_iter, 40);
    assert_eq!(solve.bracket, Some([0.0, 1.0e6]));

    let agg: &Aggregation = &run.aggregations[0];
    assert_eq!(agg.op, AggregationOp::WeightedMean);
    assert_eq!(agg.weight.as_deref(), Some("num_pols_if"));
    assert_eq!(agg.over_t, Some(OverT::Total));
    // The key tuple keeps its declared order: a drill-down is built from its prefixes.
    assert_eq!(agg.group_by, vec!["product_code", "entry_year_band"]);
}

#[test]
fn the_spec_defaults_are_applied_exactly_once() {
    let minimal = r#"format = "pir/1"

[run]
product = "p"
modelpoints = "m.csv"
out = "runs/x"
"#;
    let run = run_file(&parse(minimal)).unwrap();
    assert_eq!(run.run.emit, Emit::Outputs);
    assert_eq!(run.run.retain, Retain::Ring);
    assert_eq!(run.run.on_trap, OnTrap::Abort);
    assert_eq!(run.run.max_errors, 100);
    assert_eq!(run.run.exec.chunk_size, 1024);
    assert!(run.run.exec.progress);
    assert!(run.solves.is_empty() && run.aggregations.is_empty());
}

#[test]
fn an_unknown_enum_spelling_is_refused_not_defaulted() {
    let text = r#"format = "pir/1"

[run]
product = "p"
modelpoints = "m.csv"
out = "runs/x"
on_trap = "contnue"
"#;
    let err = run_file(&parse(text)).unwrap_err();
    assert!(err.to_string().contains("contnue"), "{err}");
}

#[test]
fn run_digest_ignores_exec_and_out_but_not_emit() {
    // Q1's partition, checked on the digest the runner actually stamps.
    let base = run_file(&parse(FULL_RUN)).unwrap();
    let mut other = base.clone();
    other.run.out = "somewhere/else".to_string();
    other.run.exec.threads = Some(1);
    other.run.exec.chunk_size = 1;
    assert_eq!(base.digest_payload(), other.digest_payload());

    let mut emit_changed = base.clone();
    emit_changed.run.emit = Emit::All;
    assert_ne!(base.digest_payload(), emit_changed.digest_payload());
}

#[test]
fn assumption_values_take_the_numbers_and_leave_the_rest() {
    let text = r#"format = "pir/1"
assumption_set = "base"

valuation_rate = 0.035
expense_per_policy = 42
mortality_table = "sa8990"
"#;
    let values = assumption_values(&parse(text));
    assert_eq!(values["valuation_rate"], 0.035);
    assert_eq!(values["expense_per_policy"], 42.0);
    assert!(!values.contains_key("mortality_table"));
}

const CSV: &str = "policy_number,product_code,sum_assured,premium,q,in_force\n\
POL1,TERM_UK,100000,900,0.01,true\n\
POL2,TERM_UK,250000,1500,0.02,true\n\
POL3,TERM_IE,50000,400,0.005,true\n";

#[test]
fn modelpoints_load_into_chunks_the_kernel_can_run() {
    let m = model(TERM);
    // `from_module`, not `pruned_for_module`: the planner allocates a `PerMP` input slot for
    // every declared field, and the kernel binds all of them, so a pruned load would leave a
    // slot unbound. See the crate docs on `load_chunks`.
    let schema = MpSchema::from_module(&m.modules[0]).unwrap();
    let mut source = CsvSource::from_reader(schema, Box::new(CSV.as_bytes()), "term.csv").unwrap();
    let chunks = load_chunks(&mut source, "policy_number", 2).unwrap();

    // Chunk boundaries are a function of `C` and the row count only.
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].keys, vec!["POL1".to_string(), "POL2".to_string()]);
    assert_eq!(chunks[1].index, 1);
    assert_eq!(chunks[1].first_row, 2);

    let run = minimal_run_file();
    let r = runner(&m, &run);
    let p = r
        .project(&chunks, &SerialExecutor, &CancelFlag::new())
        .unwrap();
    assert_eq!(p.modelpoints_projected, 3);
    // The `str` product code survived the load, and the `bool` became a 0/1 lane.
    let bels = p.per_mp("term.bel");
    assert_eq!(bels.len(), 3);
    assert_eq!(bels[0].0, "POL1");
}

#[test]
fn a_portfolio_solve_drives_an_aggregation_to_its_target() {
    let m = model(TERM);
    let mut run = minimal_run_file();
    run.aggregations = vec![Aggregation {
        name: "bel_total".to_string(),
        group_by: vec![],
        measure: "bel".to_string(),
        op: AggregationOp::Sum,
        weight: None,
        filter: None,
        over_t: None,
    }];
    run.solves = vec![Solve {
        name: "portfolio_premium".to_string(),
        target: "bel_total".to_string(),
        to: 0.0,
        vary: "premium".to_string(),
        scope: SolveScope::Portfolio,
        tolerance: 1e-7,
        max_iter: 60,
        method: "brent".to_string(),
        bracket: Some([0.0, 1.0e6]),
        on_not_converged: None,
    }];

    let mut r = runner(&m, &run);
    let mut chunks = common::term_chunks(2);
    let result = predictable_runner::solver::solve(
        &mut r,
        &run.solves[0],
        &mut chunks,
        &SerialExecutor,
        &CancelFlag::new(),
    )
    .unwrap();

    assert!(result.converged(), "{:?}", result.outcome);
    let solved = result.portfolio_value.unwrap();
    assert!(solved > 0.0 && solved < 1.0e6);
    // One scalar solve, no side file (§8.4.4).
    assert!(result.rows.is_empty());
    assert_eq!(result.outcome.scope, "portfolio");
    assert!(result.outcome.residual.max_abs <= 1e-7);

    // The portfolio really is at target with the solved premium in place.
    let p = r
        .project(&chunks, &SerialExecutor, &CancelFlag::new())
        .unwrap();
    assert!(p.aggregate("bel_total", None).unwrap().abs() < 1e-6);
}
