//! The `[[solve]]` outer loop and its manifest contract (`01-ir.md` §8.4.4 — ruling Q8).
//!
//! The solver re-executes the chunk pipeline with a perturbed input and Brent-steps the scalar
//! residual `target - to`. It never reaches inside the kernel: `predictable-engine` stays a pure
//! function, and every iteration is a whole, deterministic projection.
//!
//! What comes out is a [`SolveResult`] carrying exactly what §8.4.4 makes normative — converged
//! and not-converged counts, iteration statistics, the maximum absolute residual *and the
//! modelpoint it belongs to*, and the per-modelpoint rows of `solves/<name>.parquet`. A solved
//! run whose solve outcome is not recorded is not reproducible, which is why none of this is
//! optional.

use std::collections::BTreeMap;

use predictable_io::outbound::manifest::{IterationStats, ResidualStats, SolveOutcome};
use predictable_io::outbound::SolveRow;
use predictable_ir::run::{OnNotConverged, Solve, SolveScope};

use crate::cancel::CancelFlag;
use crate::chunk::{Chunk, ChunkColumn};
use crate::error::RunError;
use crate::executor::ChunkExecutor;
use crate::pipeline::Runner;
use crate::solve::{BrentState, Status};

/// The outcome of one `[[solve]]`.
#[derive(Debug, Clone)]
pub struct SolveResult {
    /// The manifest block of §8.4.4, complete but for the `per_mp` file reference the writer
    /// fills in once the side file exists.
    pub outcome: SolveOutcome,
    /// `solves/<name>.parquet` rows, in modelpoint file order. Empty for a portfolio solve.
    pub rows: Vec<SolveRow>,
    /// The solved value per modelpoint key — also materialised as a `PerMP` component named
    /// after the solve, so downstream joins do not special-case solves.
    pub solved: BTreeMap<String, f64>,
    /// The portfolio solve's single solved value.
    pub portfolio_value: Option<f64>,
}

impl SolveResult {
    /// True when every modelpoint met tolerance.
    pub fn converged(&self) -> bool {
        self.outcome.not_converged == 0
    }

    /// `E0903` when the solve did not converge and the policy says that is an error.
    pub fn check(&self, solve: &Solve) -> Result<(), RunError> {
        if self.converged() || solve.effective_on_not_converged() == OnNotConverged::Warn {
            return Ok(());
        }
        Err(RunError::NotConverged {
            solve: solve.name.clone(),
            modelpoints: self.outcome.not_converged,
        })
    }
}

/// Run one `[[solve]]` to convergence, leaving `chunks` holding the solved inputs.
///
/// `scope = "per_mp"` advances every modelpoint's own Brent state **in lockstep**: one pipeline
/// pass evaluates the residual for every modelpoint at its own candidate `x`. Modelpoints are
/// independent (`01-ir.md` §8.3), so this changes no answer — it turns `max_iter × n` projections
/// into at most `max_iter` of them.
pub fn solve(
    runner: &mut Runner<'_>,
    spec: &Solve,
    chunks: &mut [Chunk],
    executor: &dyn ChunkExecutor,
    cancel: &CancelFlag,
) -> Result<SolveResult, RunError> {
    match spec.scope {
        SolveScope::PerMp => solve_per_mp(runner, spec, chunks, executor, cancel),
        SolveScope::Portfolio => solve_portfolio(runner, spec, chunks, executor, cancel),
    }
}

fn bracket(spec: &Solve) -> Result<[f64; 2], RunError> {
    spec.bracket.ok_or_else(|| RunError::BadSolve {
        solve: spec.name.clone(),
        reason: "method = \"brent\" requires a `bracket`".to_string(),
    })
}

fn solve_per_mp(
    runner: &mut Runner<'_>,
    spec: &Solve,
    chunks: &mut [Chunk],
    executor: &dyn ChunkExecutor,
    cancel: &CancelFlag,
) -> Result<SolveResult, RunError> {
    let bracket = bracket(spec)?;
    // `vary` must be a modelpoint field: a `Scalar` assumption has one value for the whole run
    // and cannot take a different value per modelpoint.
    if !chunks
        .iter()
        .all(|c| c.columns.contains_key(&spec.vary) || c.is_empty())
    {
        return Err(RunError::BadSolve {
            solve: spec.name.clone(),
            reason: format!(
                "`vary = \"{}\"` must name a modelpoint field for scope = \"per_mp\"",
                spec.vary
            ),
        });
    }

    // One Brent state per modelpoint, addressed by (chunk, lane) and identified by key.
    let mut keys: Vec<String> = Vec::new();
    let mut rows: Vec<u64> = Vec::new();
    let mut sites: Vec<(usize, usize)> = Vec::new();
    for (ci, chunk) in chunks.iter().enumerate() {
        for (lane, key) in chunk.keys.iter().enumerate() {
            keys.push(key.clone());
            rows.push(chunk.first_row + lane as u64);
            sites.push((ci, lane));
        }
    }
    let mut states: Vec<BrentState> =
        vec![BrentState::new(bracket, spec.tolerance, spec.max_iter); keys.len()];

    let mut passes = 0u32;
    while states.iter().any(|s| !s.status().is_done()) && passes <= spec.max_iter + 1 {
        passes += 1;
        for (i, (ci, lane)) in sites.iter().enumerate() {
            if states[i].status().is_done() {
                continue;
            }
            set_lane(&mut chunks[*ci], &spec.vary, *lane, states[i].proposal());
        }
        let projection = runner.project(chunks, executor, cancel)?;
        let values: BTreeMap<String, f64> = projection.per_mp(&spec.target).into_iter().collect();
        for (i, key) in keys.iter().enumerate() {
            if states[i].status().is_done() {
                continue;
            }
            match values.get(key) {
                Some(v) => {
                    states[i].step(v - spec.to);
                }
                // The modelpoint trapped out of the result set; it has no residual and cannot
                // converge. Say so rather than iterating on a value that does not exist.
                None => {
                    while !states[i].status().is_done() {
                        states[i].step(f64::INFINITY);
                    }
                }
            }
        }
    }

    // Leave the chunks holding the solved inputs, so the final projection is the solved one.
    for (i, (ci, lane)) in sites.iter().enumerate() {
        set_lane(&mut chunks[*ci], &spec.vary, *lane, states[i].solution());
    }

    let mut out_rows = Vec::with_capacity(keys.len());
    let mut solved = BTreeMap::new();
    for (i, key) in keys.iter().enumerate() {
        let s = &states[i];
        let converged = s.status() == Status::Converged;
        out_rows.push(SolveRow {
            mp_key: key.clone(),
            mp_row: rows[i] as u32,
            solved_value: s.solution(),
            residual: s.residual(),
            iterations: s.iterations(),
            converged,
        });
        solved.insert(key.clone(), s.solution());
    }

    Ok(SolveResult {
        outcome: outcome_of(spec, &out_rows),
        rows: out_rows,
        solved,
        portfolio_value: None,
    })
}

fn solve_portfolio(
    runner: &mut Runner<'_>,
    spec: &Solve,
    chunks: &mut [Chunk],
    executor: &dyn ChunkExecutor,
    cancel: &CancelFlag,
) -> Result<SolveResult, RunError> {
    let bracket = bracket(spec)?;
    let varies_field = chunks.iter().any(|c| c.columns.contains_key(&spec.vary));
    let mut state = BrentState::new(bracket, spec.tolerance, spec.max_iter);

    while !state.status().is_done() {
        let x = state.proposal();
        if varies_field {
            for chunk in chunks.iter_mut() {
                for lane in 0..chunk.len() {
                    set_lane(chunk, &spec.vary, lane, x);
                }
            }
        } else {
            runner.set_assumption(&spec.vary, x);
        }
        let projection = runner.project(chunks, executor, cancel)?;
        // `target` names an `[[aggregation]]` for a portfolio solve (§8.4.4); a `PerMP` target
        // would need a rule for combining modelpoints, and that rule is the aggregation.
        let value = projection
            .aggregate(&spec.target, None)
            .ok_or_else(|| RunError::BadSolve {
                solve: spec.name.clone(),
                reason: format!(
                    "`target = \"{}\"` must name an [[aggregation]] for scope = \"portfolio\"",
                    spec.target
                ),
            })?;
        state.step(value - spec.to);
    }

    // Leave the solved input in place for the final projection.
    if varies_field {
        for chunk in chunks.iter_mut() {
            for lane in 0..chunk.len() {
                set_lane(chunk, &spec.vary, lane, state.solution());
            }
        }
    } else {
        runner.set_assumption(&spec.vary, state.solution());
    }

    let converged = state.status() == Status::Converged;
    Ok(SolveResult {
        outcome: SolveOutcome {
            name: spec.name.clone(),
            target: spec.target.clone(),
            to: spec.to,
            vary: spec.vary.clone(),
            scope: "portfolio".to_string(),
            method: spec.method.clone(),
            tolerance: spec.tolerance,
            max_iter: spec.max_iter,
            bracket: spec.bracket,
            converged: u64::from(converged),
            not_converged: u64::from(!converged),
            iterations: IterationStats {
                min: state.iterations(),
                max: state.iterations(),
                mean: f64::from(state.iterations()),
                total: u64::from(state.iterations()),
            },
            residual: ResidualStats {
                max_abs: state.residual().abs(),
                argmax_mp: None,
            },
            per_mp: None,
            on_not_converged: on_not_converged(spec),
        },
        rows: Vec::new(),
        solved: BTreeMap::new(),
        portfolio_value: Some(state.solution()),
    })
}

fn set_lane(chunk: &mut Chunk, field: &str, lane: usize, value: f64) {
    if let Some(ChunkColumn::Num(v)) = chunk.columns.get_mut(field) {
        if lane < v.len() {
            v[lane] = value;
        }
    }
}

fn on_not_converged(spec: &Solve) -> String {
    match spec.effective_on_not_converged() {
        OnNotConverged::Warn => "warn".to_string(),
        OnNotConverged::Error => "error".to_string(),
    }
}

/// The §8.4.4 manifest block, computed from the per-modelpoint rows.
fn outcome_of(spec: &Solve, rows: &[SolveRow]) -> SolveOutcome {
    let converged = rows.iter().filter(|r| r.converged).count() as u64;
    let total_iter: u64 = rows.iter().map(|r| u64::from(r.iterations)).sum();
    let (mut max_abs, mut argmax) = (0.0f64, None);
    for r in rows {
        let abs = r.residual.abs();
        if abs.is_finite() && abs > max_abs {
            max_abs = abs;
            argmax = Some(r.mp_key.clone());
        }
    }
    SolveOutcome {
        name: spec.name.clone(),
        target: spec.target.clone(),
        to: spec.to,
        vary: spec.vary.clone(),
        scope: "per_mp".to_string(),
        method: spec.method.clone(),
        tolerance: spec.tolerance,
        max_iter: spec.max_iter,
        bracket: spec.bracket,
        converged,
        not_converged: rows.len() as u64 - converged,
        iterations: IterationStats {
            min: rows.iter().map(|r| r.iterations).min().unwrap_or(0),
            max: rows.iter().map(|r| r.iterations).max().unwrap_or(0),
            mean: if rows.is_empty() {
                0.0
            } else {
                total_iter as f64 / rows.len() as f64
            },
            total: total_iter,
        },
        residual: ResidualStats {
            max_abs,
            argmax_mp: argmax,
        },
        per_mp: None,
        on_not_converged: on_not_converged(spec),
    }
}
