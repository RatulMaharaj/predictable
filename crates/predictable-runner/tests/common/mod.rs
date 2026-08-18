//! Test rig: `.pir` text → plan → tapes → a bound [`Runner`], plus chunk builders.
#![allow(dead_code)]

use std::collections::BTreeMap;

use predictable_check::Input;
use predictable_ir::run::RunFile;
use predictable_ir::Module;
use predictable_plan::{plan_sources, Plan, PlanOptions};
use predictable_runner::chunk::{Chunk, ChunkColumn};
use predictable_runner::pipeline::{RunInputs, Runner};
use predictable_tape::{lower_plan, TapeProgram};

/// A term-assurance model with a stage-2 `bel`, a bool filter and a `str` grouping field.
pub const TERM: &str = r#"
format = "pir/1"
module = "term"

[timeline]
basis = "annual"
periods = 3
origin = "policy"
valuation_date = 2026-06-30

[[modelpoint_field]]
name = "policy_number"
dtype = "str"
required = true
key = true

[[modelpoint_field]]
name = "product_code"
dtype = "str"
required = true

[[modelpoint_field]]
name = "sum_assured"
dtype = "f64"
unit = "money"
required = true

[[modelpoint_field]]
name = "premium"
dtype = "f64"
unit = "money"
required = true

[[modelpoint_field]]
name = "q"
dtype = "f64"
unit = "prob"
required = true

[[modelpoint_field]]
name = "in_force"
dtype = "bool"
required = true

[[component]]
name = "survivors"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "count"
timing = "start"
init = "1.0"
expr = "survivors[t-1] * (1 - q)"

[[component]]
name = "claims"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "survivors * q * sum_assured"

[[component]]
name = "premium_income"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "survivors * premium"

[[component]]
name = "net_cashflow"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "claims - premium_income"

[[component]]
name = "bel"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum(net_cashflow)"
"#;

/// A model whose `ratio` divides by a modelpoint field, so a zero traps (`E0902`).
pub const TRAPPING: &str = r#"
format = "pir/1"
module = "trap"

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
name = "numerator"
dtype = "f64"
unit = "money"
required = true

[[modelpoint_field]]
name = "exposure"
dtype = "f64"
unit = "count"
required = true

[[component]]
name = "ratio"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "numerator / exposure"
"#;

pub struct Model {
    pub plan: Plan,
    pub tapes: TapeProgram,
    pub modules: Vec<Module>,
}

/// Plan and lower one `.pir` source, retaining every series so any of them may be emitted.
pub fn model(src: &str) -> Model {
    let inputs = [Input::new("test.pir", src)];
    let options = PlanOptions {
        retain_all: true,
        ..PlanOptions::default()
    };
    let plan = plan_sources(&inputs, &options).expect("plan");
    let tapes = lower_plan(&plan).expect("lower");
    Model {
        plan,
        tapes,
        modules: predictable_plan::lower_modules(&inputs),
    }
}

/// Bind a runner over a model and a run file.
pub fn runner<'a>(m: &'a Model, run: &'a RunFile) -> Runner<'a> {
    Runner::new(RunInputs {
        modules: &m.modules,
        plan: &m.plan,
        tapes: &m.tapes,
        tables: vec![],
        assumptions: BTreeMap::new(),
        run,
    })
    .expect("bind runner")
}

/// One chunk from named columns. `keys` doubles as the `policy_number` text column.
pub fn chunk(index: u32, first_row: u64, keys: &[&str], columns: &[(&str, ChunkColumn)]) -> Chunk {
    let mut map: BTreeMap<String, ChunkColumn> = columns
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect();
    map.insert(
        "policy_number".to_string(),
        ChunkColumn::Text(keys.iter().map(|k| (*k).to_string()).collect()),
    );
    Chunk {
        index,
        first_row,
        keys: keys.iter().map(|k| (*k).to_string()).collect(),
        columns: map,
    }
}

pub fn nums(values: &[f64]) -> ChunkColumn {
    ChunkColumn::Num(values.to_vec())
}

pub fn text(values: &[&str]) -> ChunkColumn {
    ChunkColumn::Text(values.iter().map(|v| (*v).to_string()).collect())
}

/// Four term modelpoints, split across `chunk_size`-wide chunks.
pub fn term_chunks(chunk_size: usize) -> Vec<Chunk> {
    let keys = ["POL1", "POL2", "POL3", "POL4"];
    let product = ["TERM_UK", "TERM_UK", "TERM_IE", "TERM_IE"];
    let sum_assured = [100_000.0, 250_000.0, 50_000.0, 75_000.0];
    let premium = [900.0, 1_500.0, 400.0, 600.0];
    let q = [0.01, 0.02, 0.005, 0.03];
    let in_force = [1.0, 1.0, 1.0, 0.0];

    let mut out = Vec::new();
    let mut row = 0usize;
    let mut index = 0u32;
    while row < keys.len() {
        let end = (row + chunk_size).min(keys.len());
        out.push(chunk(
            index,
            row as u64,
            &keys[row..end],
            &[
                ("product_code", text(&product[row..end])),
                ("sum_assured", nums(&sum_assured[row..end])),
                ("premium", nums(&premium[row..end])),
                ("q", nums(&q[row..end])),
                ("in_force", nums(&in_force[row..end])),
            ],
        ));
        row = end;
        index += 1;
    }
    out
}
