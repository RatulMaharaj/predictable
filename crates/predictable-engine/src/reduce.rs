//! Stage-2 reductions: strictly sequential in `t` (`01-ir.md` §9.2).
//!
//! These are the one place in the kernel that is deliberately *not* vectorised.
//! A pairwise or SIMD sum reassociates additions, which moves the last bits of
//! the answer; an actuary reconciling to Prophet to the penny would see the
//! difference and file a ticket. So the loop is a plain left-to-right fold, and
//! the compensated variant is a separately named builtin (`sum_kahan`) that the
//! model has to ask for.

use predictable_ir::{AggOp, Timing};
use predictable_plan::SlotId;

use crate::buffers::ChunkBuffers;
use crate::exec::{Ctx, RunState};
use crate::layout::Place;
use crate::ops;

/// One lane of a series at period `t`, wherever it is stored.
pub(crate) fn value_at(
    ctx: &Ctx<'_>,
    state: &RunState,
    bufs: &ChunkBuffers,
    slot: SlotId,
    t: u32,
    lane: usize,
) -> f64 {
    match ctx.layout.place(slot) {
        Place::Hoisted { offset, .. } => state.hoisted[offset + t as usize],
        Place::PerMp { offset } => bufs.permp[offset + lane],
        Place::Scalar { index } => state.scalars[index],
        Place::Series { .. } => {
            let base = ctx.layout.series_at(slot, t).unwrap_or(0);
            bufs.series[base + lane]
        }
    }
}

/// `sum` / `sum_kahan` / `first` / `last` / `max_over` / `min_over` /
/// `count_while`, with the optional predicate of `01-ir.md` §2.8.
#[allow(clippy::too_many_arguments)]
#[inline(never)] // never vectorise, never reassociate (`01-ir.md` §9.2)
pub(crate) fn reduce(
    ctx: &Ctx<'_>,
    state: &RunState,
    bufs: &ChunkBuffers,
    agg: AggOp,
    series: SlotId,
    pred: Option<SlotId>,
    lane: usize,
) -> f64 {
    let t_max = ctx.layout.periods;
    let keep = |t: u32| match pred {
        None => true,
        Some(p) => ops::truthy(value_at(ctx, state, bufs, p, t, lane)),
    };
    let get = |t: u32| value_at(ctx, state, bufs, series, t, lane);

    match agg {
        AggOp::Sum => {
            let mut acc = 0.0;
            for t in 0..=t_max {
                if keep(t) {
                    acc += get(t);
                }
            }
            acc
        }
        // Neumaier-style compensation, still strictly in `t` order.
        AggOp::SumKahan => {
            let (mut acc, mut c) = (0.0f64, 0.0f64);
            for t in 0..=t_max {
                if !keep(t) {
                    continue;
                }
                let x = get(t);
                let sum = acc + x;
                c += if acc.abs() >= x.abs() {
                    (acc - sum) + x
                } else {
                    (x - sum) + acc
                };
                acc = sum;
            }
            acc + c
        }
        AggOp::First | AggOp::At => (0..=t_max).find(|&t| keep(t)).map(get).unwrap_or(0.0),
        AggOp::Last => (0..=t_max).rev().find(|&t| keep(t)).map(get).unwrap_or(0.0),
        AggOp::MaxOver => (0..=t_max)
            .filter(|&t| keep(t))
            .map(get)
            .fold(f64::NEG_INFINITY, |a, b| if b > a { b } else { a }),
        AggOp::MinOver => (0..=t_max)
            .filter(|&t| keep(t))
            .map(get)
            .fold(f64::INFINITY, |a, b| if b < a { b } else { a }),
        // Stops at the first false (§2.8) — it does not count all true periods.
        AggOp::CountWhile => {
            let mut n = 0.0;
            for t in 0..=t_max {
                if !ops::truthy(get(t)) {
                    break;
                }
                n += 1.0;
            }
            n
        }
        AggOp::Npv => 0.0, // lowered to `Op::Npv`, never reaches here
    }
}

/// `npv(value, disc)` using `value`'s timing tag (`01-ir.md` §2.5).
///
/// `disc` is a **cumulative** discount-factor series — `disc[t]` discounts a
/// cash flow at the *start* of period `t` — so `start`/`point` flows use
/// `disc[t]` directly. `end` and `mid` need the fractional step, and the only
/// honest source of it is the curve itself: the period's own one-step factor
/// `f = disc[t+1] / disc[t]`, applied as `disc[t] · f^(exponent - t)`. On a flat
/// curve, where `disc[t] = v^t`, that is exactly the spec's `v^t`, `v^(t+1)`,
/// `v^(t+0.5)` — and on a real curve it stays consistent with the curve instead
/// of silently assuming a flat one.
#[inline(never)] // never vectorise, never reassociate (`01-ir.md` §9.2)
pub(crate) fn npv(
    ctx: &Ctx<'_>,
    state: &RunState,
    bufs: &ChunkBuffers,
    value: SlotId,
    disc: SlotId,
    timing: Timing,
    lane: usize,
) -> f64 {
    let t_max = ctx.layout.periods;
    let mut acc = 0.0;
    for t in 0..=t_max {
        let x = value_at(ctx, state, bufs, value, t, lane);
        acc += x * npv_factor(ctx, state, bufs, disc, timing, t, lane);
    }
    acc
}

/// The timing-adjusted discount factor at `t`. Shared with the `explain()`
/// recorder ([`crate::terms`]) so a trace shows the factor the kernel used,
/// not a reconstruction of it.
#[inline]
pub(crate) fn npv_factor(
    ctx: &Ctx<'_>,
    state: &RunState,
    bufs: &ChunkBuffers,
    disc: SlotId,
    timing: Timing,
    t: u32,
    lane: usize,
) -> f64 {
    let t_max = ctx.layout.periods;
    let d = value_at(ctx, state, bufs, disc, t, lane);
    let frac = timing.discount_exponent(t) - f64::from(t);
    if frac == 0.0 {
        d
    } else {
        let next = if t < t_max {
            value_at(ctx, state, bufs, disc, t + 1, lane)
        } else {
            d
        };
        let step = if d == 0.0 { 1.0 } else { next / d };
        d * ops::pow(step, frac).0
    }
}
