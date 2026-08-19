//! Test rig: source text → plan → tapes → a runnable engine.
//!
//! Shared by every integration test in this crate; each uses a subset.
#![allow(dead_code)]

use std::collections::BTreeMap;

use predictable_check::Input;
use predictable_engine::{ChunkInput, ChunkOutput, Engine, EngineError, RunConfig};
use predictable_ir::{Basis, Origin, Timeline};
use predictable_plan::{plan_sources, Plan, PlanOptions};
use predictable_tables::CompiledTable;
use predictable_tape::{lower_plan, TapeProgram};

pub struct Model {
    pub plan: Plan,
    pub tapes: TapeProgram,
    pub timeline: Timeline,
}

/// Plan and lower one `.pir` source.
pub fn model(src: &str) -> Model {
    let inputs = [Input::new("test.pir", src)];
    let plan = plan_sources(&inputs, &PlanOptions::default()).expect("plan");
    let tapes = lower_plan(&plan).expect("lower");
    let timeline = predictable_plan::lower_modules(&inputs)
        .into_iter()
        .find_map(|m| m.timeline)
        .unwrap_or(Timeline {
            basis: Basis::Annual,
            periods: plan.periods,
            origin: Origin::Policy,
            valuation_date: "2026-06-30".to_string(),
            year_convention: "act/365".to_string(),
        });
    Model {
        plan,
        tapes,
        timeline,
    }
}

/// A chunk from column vectors, keyed `MP0..MPn`.
pub fn chunk(index: u32, columns: &[(&str, Vec<f64>)]) -> ChunkInput {
    let n = columns.first().map(|(_, v)| v.len()).unwrap_or(0);
    let mut columns: BTreeMap<String, Vec<f64>> = columns
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect();
    // The key column is a `str` input, so it arrives dictionary-encoded; one
    // distinct code per lane is all the kernel ever compares.
    columns
        .entry("policy_number".to_string())
        .or_insert_with(|| (0..n).map(|i| i as f64).collect());
    ChunkInput {
        index,
        keys: (0..n).map(|i| format!("MP{i}")).collect(),
        first_row: u64::from(index) * n as u64,
        columns,
    }
}

/// Run every chunk through one engine and one reused arena.
pub fn run(
    m: &Model,
    tables: Vec<CompiledTable>,
    config: RunConfig,
    assumptions: &BTreeMap<String, f64>,
    chunks: &[ChunkInput],
) -> Result<Vec<ChunkOutput>, EngineError> {
    let mut engine = Engine::new(&m.plan, &m.tapes, tables, &m.timeline, config)?;
    let mut bufs = engine.buffers();
    engine.prepare(assumptions, &mut bufs)?;
    chunks
        .iter()
        .map(|c| engine.run_chunk(&mut bufs, c))
        .collect()
}

/// The common case: no tables, no assumptions, default config.
pub fn run_simple(m: &Model, chunks: &[ChunkInput]) -> Vec<ChunkOutput> {
    run(m, vec![], RunConfig::default(), &BTreeMap::new(), chunks).expect("run")
}
