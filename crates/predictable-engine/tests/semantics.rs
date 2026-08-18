//! Op semantics: tables, reductions, accumulators, timeline fields, rounding
//! and dates.

mod common;

use std::collections::BTreeMap;

use common::{chunk, model, run, run_simple};
use predictable_engine::{dates, ops, EngineError, RunConfig, TrapKind, TrapPolicy};
use predictable_ir::{
    DType, KeyPolicy, OnMissing, TableDecl, TableKey, TableSource, TableValue, Unit,
};
use predictable_tables::{load_bytes, CompiledTable, LoadOptions, TableBytes, TableFormat};

// ---------------------------------------------------------------------------
// Tables
// ---------------------------------------------------------------------------

const LOOKUP: &str = r#"
format = "pir/1"
module = "mort"

[timeline]
basis = "annual"
periods = 2
origin = "policy"
valuation_date = 2026-06-30

[[modelpoint_field]]
name = "policy_number"
dtype = "str"
required = true
key = true

[[modelpoint_field]]
name = "entry_age"
dtype = "i64"
unit = "years"
required = true

[[modelpoint_field]]
name = "smoker"
dtype = "bool"
required = true

[[table]]
name = "sa"
keys = [
  { name = "age", dtype = "i64", policy = "clamp" },
  { name = "smoker", dtype = "bool", policy = "exact" },
]
values = [{ name = "qx", dtype = "f64", unit = "prob" }]
on_missing = "error"
source = "resource:sa"

[[component]]
name = "age"
kind = "Derived"
dtype = "i64"
shape = "Series"
unit = "years"
timing = "start"
expr = "entry_age + t"

[[component]]
name = "qx"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "prob"
timing = "end"
expr = "sa@(age, smoker)"
"#;

fn mortality(on_missing: OnMissing) -> CompiledTable {
    let decl = TableDecl {
        name: "sa".into(),
        keys: vec![
            TableKey {
                name: "age".into(),
                dtype: DType::I64,
                policy: KeyPolicy::Clamp,
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
            unit: Unit::default(),
        }],
        on_missing,
        source: TableSource::File("tables/sa.csv".into()),
        digest: None,
        rows: None,
        doc: None,
    };
    let csv = "age,smoker,qx\n40,false,0.001\n40,true,0.003\n41,false,0.0012\n41,true,0.0035\n";
    let bytes = TableBytes {
        bytes: csv.into(),
        origin: "tables/sa.csv".into(),
        format: TableFormat::Csv,
    };
    load_bytes(&decl, bytes, &LoadOptions::default()).expect("compile table")
}

#[test]
fn a_lookup_reads_the_compiled_table_per_lane() {
    let m = model(LOOKUP);
    let out = run(
        &m,
        vec![mortality(OnMissing::Error)],
        RunConfig::default(),
        &BTreeMap::new(),
        &[chunk(
            0,
            &[("entry_age", vec![40.0, 40.0]), ("smoker", vec![0.0, 1.0])],
        )],
    )
    .expect("run");
    let qx = out[0].column("mort.qx").unwrap();

    // Age advances with t; the `clamp` policy holds the top of the table at 42.
    assert_eq!(qx.lane(0), &[0.001, 0.0012, 0.0012]);
    // The bool key selects the smoker rows on the second lane.
    assert_eq!(qx.lane(1), &[0.003, 0.0035, 0.0035]);
}

#[test]
fn a_lookup_miss_under_on_missing_error_is_a_trap() {
    // `smoker` is `exact`, so a code outside {false, true} has no row.
    let m = model(LOOKUP);
    let err = run(
        &m,
        vec![mortality(OnMissing::Error)],
        RunConfig::default(),
        &BTreeMap::new(),
        &[chunk(
            0,
            &[("entry_age", vec![40.0]), ("smoker", vec![7.0])],
        )],
    )
    .expect_err("must trap");
    let EngineError::Trapped(log) = err else {
        panic!("expected a lookup trap");
    };
    assert_eq!(log.reports()[0].kind, TrapKind::LookupMiss);
    assert_eq!(log.reports()[0].component, "mort.qx");
    assert_eq!(log.reports()[0].envelope()["trap"], "lookup_miss");
}

#[test]
fn on_missing_default_substitutes_instead_of_trapping() {
    let m = model(LOOKUP);
    let out = run(
        &m,
        vec![mortality(OnMissing::Default(
            predictable_ir::LitValue::Float(0.5),
        ))],
        RunConfig::default(),
        &BTreeMap::new(),
        &[chunk(
            0,
            &[("entry_age", vec![40.0]), ("smoker", vec![7.0])],
        )],
    )
    .expect("no trap");
    assert_eq!(out[0].column("mort.qx").unwrap().lane(0), &[0.5, 0.5, 0.5]);
    assert!(out[0].traps.is_empty());
}

// ---------------------------------------------------------------------------
// Stage 2 reductions
// ---------------------------------------------------------------------------

const REDUCE: &str = r#"
format = "pir/1"
module = "agg"

[timeline]
basis = "annual"
periods = 4
origin = "policy"
valuation_date = 2026-06-30

[[modelpoint_field]]
name = "policy_number"
dtype = "str"
required = true
key = true

[[modelpoint_field]]
name = "amount"
dtype = "f64"
unit = "money"
required = true

[[component]]
name = "flow"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "amount * (t + 1)"

[[component]]
name = "running"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "cum(flow)"

[[component]]
name = "total"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum(flow)"

[[component]]
name = "peak"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "max_over(flow)"

[[component]]
name = "trough"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "min_over(flow)"

[[component]]
name = "compensated"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum_kahan(flow)"

[[component]]
name = "opening"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "first(flow)"

[[component]]
name = "closing"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "last(flow)"
"#;

#[test]
fn reductions_run_sequentially_over_the_whole_series() {
    let m = model(REDUCE);
    let out = run_simple(&m, &[chunk(0, &[("amount", vec![10.0, 1.0])])]);
    let out = &out[0];

    // flow = 10, 20, 30, 40, 50
    assert_eq!(out.column("agg.total").unwrap().lane(0)[0], 150.0);
    assert_eq!(out.column("agg.peak").unwrap().lane(0)[0], 50.0);
    assert_eq!(out.column("agg.trough").unwrap().lane(0)[0], 10.0);
    assert_eq!(out.column("agg.opening").unwrap().lane(0)[0], 10.0);
    assert_eq!(out.column("agg.closing").unwrap().lane(0)[0], 50.0);
    // `sum_kahan` is the only compensated variant, and agrees here exactly.
    assert_eq!(out.column("agg.compensated").unwrap().lane(0)[0], 150.0);
    // `cum` carries its accumulator across periods in the arena.
    assert_eq!(
        out.column("agg.running").unwrap().lane(0),
        &[10.0, 30.0, 60.0, 100.0, 150.0]
    );
    // The second lane is untouched by the first.
    assert_eq!(out.column("agg.total").unwrap().lane(1)[0], 15.0);
}

#[test]
fn accumulators_do_not_leak_between_chunks() {
    let m = model(REDUCE);
    let out = run_simple(
        &m,
        &[
            chunk(0, &[("amount", vec![10.0])]),
            chunk(1, &[("amount", vec![10.0])]),
        ],
    );
    assert_eq!(
        out[0].column("agg.running").unwrap().lane(0),
        out[1].column("agg.running").unwrap().lane(0)
    );
}

// ---------------------------------------------------------------------------
// Timeline fields
// ---------------------------------------------------------------------------

const CLOCK: &str = r#"
format = "pir/1"
module = "clk"

[timeline]
basis = "monthly"
periods = 13
origin = "policy"
valuation_date = 2026-01-31

[[modelpoint_field]]
name = "policy_number"
dtype = "str"
required = true
key = true

[[component]]
name = "py"
kind = "Output"
dtype = "i64"
shape = "Series"
unit = "years"
timing = "start"
expr = "policy_year"

[[component]]
name = "anniv"
kind = "Output"
dtype = "bool"
shape = "Series"
unit = "none"
timing = "start"
expr = "if is_anniversary then 1.0 else 0.0"

[[component]]
name = "month"
kind = "Output"
dtype = "i64"
shape = "Series"
unit = "none"
timing = "start"
expr = "month_of_year"
"#;

#[test]
fn timeline_fields_are_computed_not_stored() {
    let m = model(CLOCK);
    let out = run_simple(&m, &[chunk(0, &[("policy_number", vec![0.0])])]);
    let out = &out[0];

    let py = out.column("clk.py").unwrap();
    assert_eq!(py.lane(0)[0], 1.0);
    assert_eq!(py.lane(0)[11], 1.0);
    assert_eq!(py.lane(0)[12], 2.0, "policy year rolls at t = 12");

    let anniv = out.column("clk.anniv").unwrap();
    assert_eq!(anniv.lane(0)[0], 0.0, "t = 0 is not an anniversary");
    assert_eq!(anniv.lane(0)[12], 1.0);

    // 2026-01-31 + 1 month clamps to 2026-02-28, so the month advances by one.
    let month = out.column("clk.month").unwrap();
    assert_eq!(month.lane(0)[0], 1.0);
    assert_eq!(month.lane(0)[1], 2.0);
    assert_eq!(month.lane(0)[12], 1.0);
}

// ---------------------------------------------------------------------------
// Builtins
// ---------------------------------------------------------------------------

#[test]
fn round_is_half_away_from_zero_on_the_shortest_decimal() {
    // The `2.675` trap (`01-ir.md` §2.8): IEEE half-even on the binary value
    // gives 2.67, the actuarial convention gives 2.68.
    assert_eq!(ops::round_half_away(2.675, 2), 2.68);
    assert_eq!(ops::round_half_away(-2.675, 2), -2.68);
    assert_eq!(ops::round_half_away(0.5, 0), 1.0);
    assert_eq!(ops::round_half_away(1.5, 0), 2.0);
    assert_eq!(ops::round_half_away(2.5, 0), 3.0, "not half-even");
    assert_eq!(ops::round_half_away(-0.5, 0), -1.0);
    assert_eq!(ops::round_half_away(9.99, 1), 10.0, "carry propagates");
    assert_eq!(ops::round_half_away(1.0, 4), 1.0);
    assert!(ops::round_half_away(f64::NAN, 2).is_nan());
}

#[test]
fn ln_of_a_non_positive_value_traps_rather_than_producing_nan() {
    assert_eq!(
        ops::apply1(predictable_tape::Fn1::Ln, 0.0).1,
        Some(TrapKind::LogNonPositive)
    );
    assert_eq!(ops::apply1(predictable_tape::Fn1::Ln, 1.0), (0.0, None));
    assert_eq!(ops::div(1.0, 0.0).1, Some(TrapKind::DivByZero));
    assert_eq!(ops::pow(-1.0, 0.5).1, Some(TrapKind::PowNan));
}

#[test]
fn date_arithmetic_is_exact_and_host_independent() {
    assert_eq!(dates::parse_iso("1970-01-01"), Some(0));
    assert_eq!(dates::parse_iso("2026-06-30"), Some(20634));
    let d = dates::parse_iso("2026-01-31").unwrap();
    // A 31st into February clamps rather than overflowing into March.
    assert_eq!(
        dates::civil_from_days(dates::add_months(d, 1)),
        (2026, 2, 28)
    );
    assert_eq!(
        dates::civil_from_days(dates::add_months(d, 12)),
        (2027, 1, 31)
    );
    assert_eq!(
        dates::civil_from_days(dates::add_months(d, -1)),
        (2025, 12, 31)
    );
    assert_eq!(dates::months_between(d, dates::add_months(d, 7)), 7);
    assert!(dates::is_leap(2024) && !dates::is_leap(2100));
}

// ---------------------------------------------------------------------------
// Policy plumbing
// ---------------------------------------------------------------------------

#[test]
fn abort_is_the_default_policy() {
    assert_eq!(RunConfig::default().on_trap, TrapPolicy::Abort);
    // `01-ir.md` §9.3.1 fixes the retention cap at 100.
    assert_eq!(RunConfig::default().max_errors, 100);
    assert_eq!(RunConfig::default().chunk, 1024);
}
