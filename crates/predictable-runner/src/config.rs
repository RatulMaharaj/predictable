//! Lowering a parsed `.pir` run file to the IR's [`RunFile`] (`01-ir.md` §8.4.2–§8.4.4).
//!
//! The parser ([`predictable_syntax`]) produces an AST with every key optional, because its job
//! is to recover from bad input and report on it. The runner needs the *typed* configuration,
//! with the spec's defaults applied exactly once. Applying them here, rather than at each use
//! site, is what makes `run_digest`'s partition — "a field participates iff changing it can
//! change a number" — checkable against a single struct.
//!
//! Defaults, all from §8.4.2: `emit = "outputs"`, `retain = "ring"`,
//! `storage_precision = "f64"`, `on_trap = "abort"`, `max_errors = 100`,
//! `allow_table_drift = false`, `sum_kahan = false`, `chunk_size = 1024`, `progress = true`.

use std::collections::BTreeMap;

use predictable_ir::run::{
    Aggregation, AggregationOp, Emit, ExecConfig, OnTrap, OverT, Retain, RunConfig, RunFile, Solve,
    SolveScope, StoragePrecision,
};
use predictable_syntax::ast::PirDocument;
use predictable_syntax::raw::Value;

use crate::error::RunError;

/// Lower a parsed run document. Unknown enum spellings are refused rather than defaulted —
/// silently reading `on_trap = "contnue"` as `abort` is exactly the class of bug the IR's
/// closed enums exist to prevent.
pub fn run_file(doc: &PirDocument) -> Result<RunFile, RunError> {
    let run = doc.run.as_ref().ok_or_else(|| bad("no [run] block"))?;
    let config = RunConfig {
        product: run.product.clone().unwrap_or_default(),
        assumptions: run.assumptions.clone(),
        modelpoints: run.modelpoints.clone().unwrap_or_default(),
        out: run.out.clone().unwrap_or_default(),
        emit: match run.emit.as_deref() {
            None | Some("outputs") => Emit::Outputs,
            Some("all") => Emit::All,
            Some("list") => Emit::List,
            Some(other) => return Err(bad(&format!("emit = \"{other}\" is not a valid mode"))),
        },
        emit_list: run.emit_list.clone(),
        retain: match run.retain.as_deref() {
            None | Some("ring") => Retain::Ring,
            Some("full") => Retain::Full,
            Some(other) => return Err(bad(&format!("retain = \"{other}\" is not a valid mode"))),
        },
        storage_precision: match run.storage_precision.as_deref() {
            None | Some("f64") => StoragePrecision::F64,
            // Q15: parsed so the key exists in the grammar, refused because IR 1.0 is `f64`.
            Some("f32") => StoragePrecision::F32,
            Some(other) => {
                return Err(bad(&format!(
                    "storage_precision = \"{other}\" is not a valid precision"
                )))
            }
        },
        on_trap: match run.on_trap.as_deref() {
            None | Some("abort") => OnTrap::Abort,
            Some("continue") => OnTrap::Continue,
            Some(other) => return Err(bad(&format!("on_trap = \"{other}\" is not a policy"))),
        },
        max_errors: run.max_errors.unwrap_or(100).max(0) as u32,
        allow_table_drift: run.allow_table_drift.unwrap_or(false),
        sum_kahan: run.sum_kahan.unwrap_or(false),
        tables: run.tables.iter().cloned().collect(),
        exec: ExecConfig {
            threads: run.exec.threads.map(|t| t.max(1) as u32),
            chunk_size: run.exec.chunk_size.unwrap_or(1024).max(1) as u32,
            progress: run.exec.progress.unwrap_or(true),
        },
    };

    let solves = doc
        .solves
        .iter()
        .map(|s| {
            Ok(Solve {
                name: s.name.clone(),
                target: s.target.clone(),
                to: s.to,
                vary: s.vary.clone(),
                scope: match s.scope.as_deref() {
                    None | Some("per_mp") => SolveScope::PerMp,
                    Some("portfolio") => SolveScope::Portfolio,
                    Some(other) => return Err(bad(&format!("scope = \"{other}\" is not a scope"))),
                },
                tolerance: s.tolerance.unwrap_or(1e-8),
                max_iter: s.max_iter.unwrap_or(50).max(1) as u32,
                method: s.method.clone().unwrap_or_else(|| "brent".to_string()),
                bracket: s.bracket.map(|(lo, hi)| [lo, hi]),
                on_not_converged: None,
            })
        })
        .collect::<Result<Vec<_>, RunError>>()?;

    let aggregations = doc
        .aggregations
        .iter()
        .map(|a| {
            Ok(Aggregation {
                name: a.name.clone(),
                group_by: a.group_by.clone(),
                measure: a.measure.clone(),
                op: match a.op.as_str() {
                    "sum" => AggregationOp::Sum,
                    "mean" => AggregationOp::Mean,
                    "min" => AggregationOp::Min,
                    "max" => AggregationOp::Max,
                    "count" => AggregationOp::Count,
                    "weighted_mean" => AggregationOp::WeightedMean,
                    other => return Err(bad(&format!("op = \"{other}\" is not a monoid"))),
                },
                weight: a.weight.clone(),
                filter: a.filter.clone(),
                over_t: match a.over_t.as_deref() {
                    None => None,
                    Some("each") => Some(OverT::Each),
                    Some("total") => Some(OverT::Total),
                    Some(other) => return Err(bad(&format!("over_t = \"{other}\" is not valid"))),
                },
            })
        })
        .collect::<Result<Vec<_>, RunError>>()?;

    Ok(RunFile {
        format: doc.format.clone().unwrap_or_else(|| "pir/1".to_string()),
        run: config,
        solves,
        aggregations,
    })
}

/// The numeric values of a parsed assumption-set file, as the kernel's prologue wants them.
///
/// Non-numeric assumptions are skipped rather than coerced: an assumption whose value is a
/// string is a table key or an enum, and guessing a number for it would be a wrong answer
/// wearing the costume of a right one.
pub fn assumption_values(doc: &PirDocument) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    for (name, value) in &doc.assumption_values {
        let v = match value {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            Value::Bool(b) => Some(f64::from(u8::from(*b))),
            _ => None,
        };
        if let Some(v) = v {
            out.insert(name.value.clone(), v);
        }
    }
    out
}

fn bad(reason: &str) -> RunError {
    RunError::BadSolve {
        solve: "[run]".to_string(),
        reason: reason.to_string(),
    }
}
