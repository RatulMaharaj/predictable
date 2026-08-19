//! The interpreter: one tape, all `C` lanes, one period.
//!
//! The whole kernel is this file's `for op in tape` loop. Two properties are
//! worth stating because everything else follows from them:
//!
//! * **The innermost loop is over lanes.** Dispatch on `Op` is paid once per
//!   chunk instead of once per modelpoint (§4.2), and the lane loop is
//!   element-wise, so a vectorising compiler cannot reassociate anything and
//!   cannot change a bit.
//! * **The same code runs the replay.** A trap replays the *same tape* for one
//!   lane ([`Mode::Replay`]), so the operands in an `E0902` report are the values
//!   the run really computed, not a reconstruction (§5.5). Re-executing a period
//!   is safe precisely because the kernel is deterministic: every store rewrites
//!   the identical bits.

use predictable_plan::{Plan, SlotId};
use predictable_tables::{CompiledTable, KeyArg, KeyIndexKind, Outcome as TableOutcome};
use predictable_tape::{Op, Reg, SiteId, Tape};

use crate::buffers::ChunkBuffers;
use crate::dict::Dictionary;
use crate::layout::{Layout, Place};
use crate::ops;
use crate::timeline::Clock;
use crate::traps::TrapKind;

/// Which compiled table and value column a `TableId` names.
#[derive(Debug, Clone, Copy)]
pub struct TableBinding {
    pub table: usize,
    pub value: usize,
}

/// Everything a tape execution reads and never writes.
pub(crate) struct Ctx<'a> {
    pub plan: &'a Plan,
    pub layout: &'a Layout,
    pub clock: &'a Clock,
    pub compiled: &'a [CompiledTable],
    pub bindings: &'a [TableBinding],
    pub dict: &'a Dictionary,
}

/// Run-level state: computed once, shared by every chunk.
#[derive(Debug, Clone, Default)]
pub struct RunState {
    /// `Scalar` slots.
    pub scalars: Vec<f64>,
    /// Loop-invariant series, `(T+1)` per hoisted slot (§3.4).
    pub hoisted: Vec<f64>,
    /// `init` seeds of hoisted series.
    pub hoisted_seeds: Vec<f64>,
}

impl RunState {
    pub fn new(layout: &Layout) -> RunState {
        RunState {
            scalars: vec![0.0; layout.scalar_len],
            hoisted: vec![0.0; layout.hoisted_len],
            hoisted_seeds: vec![0.0; layout.hoisted_seed_len],
        }
    }
}

/// What the replay captured: the first trap on the lane, with its operands.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TrapHit {
    pub kind: TrapKind,
    pub site: Option<SiteId>,
    pub op_index: usize,
    pub operands: Vec<(String, f64)>,
}

/// Lane mode. `Lanes` is the hot path; `Replay` is the diagnostic path.
pub(crate) enum Mode<'r> {
    Lanes(usize),
    Replay {
        lane: usize,
        hit: &'r mut Option<TrapHit>,
    },
}

impl Mode<'_> {
    fn range(&self) -> (usize, usize) {
        match self {
            Mode::Lanes(n) => (0, *n),
            Mode::Replay { lane, .. } => (*lane, *lane + 1),
        }
    }

    fn is_replay(&self) -> bool {
        matches!(self, Mode::Replay { .. })
    }
}

/// Execute one tape over one period.
///
/// Returns `false` only in replay mode, when the trap has been captured and
/// there is nothing more to learn from the rest of the tape.
pub(crate) fn exec_tape(
    ctx: &Ctx<'_>,
    state: &mut RunState,
    bufs: &mut ChunkBuffers,
    tape: &Tape,
    t: u32,
    mode: &mut Mode<'_>,
    ops_range: std::ops::Range<usize>,
) -> bool {
    let cap = ctx.layout.chunk;
    let (lo, hi) = mode.range();
    let replay = mode.is_replay();

    macro_rules! r {
        ($reg:expr, $c:expr) => {
            bufs.regs[$reg.0 as usize * cap + $c]
        };
    }

    let start = ops_range.start;
    for (offset, op) in tape.ops[ops_range].iter().enumerate() {
        let index = start + offset;
        match op {
            Op::ConstF(d, c) => {
                let v = tape.const_f[c.0 as usize];
                for lane in lo..hi {
                    r!(d, lane) = v;
                }
            }
            Op::ConstI(d, c) => {
                let v = tape.const_i[c.0 as usize] as f64;
                for lane in lo..hi {
                    r!(d, lane) = v;
                }
            }
            Op::ConstS(d, c) => {
                let v = ctx.dict.code_of(&tape.const_s[c.0 as usize]);
                for lane in lo..hi {
                    r!(d, lane) = v;
                }
            }
            Op::LoadScalar(d, s) => {
                let v = match ctx.layout.place(*s) {
                    Place::Scalar { index } => state.scalars[index],
                    _ => 0.0,
                };
                for lane in lo..hi {
                    r!(d, lane) = v;
                }
            }
            Op::LoadPerMp(d, s) => {
                let Place::PerMp { offset } = ctx.layout.place(*s) else {
                    continue;
                };
                debug_assert!(
                    bufs.is_written(offset),
                    "per-modelpoint slot read before it was written ({})",
                    ctx.plan.info(*s).name
                );
                for lane in lo..hi {
                    r!(d, lane) = bufs.permp[offset + lane];
                }
            }
            Op::LoadCur(d, s) => {
                let base = series_base(ctx, *s, t);
                for lane in lo..hi {
                    let v = read(ctx, state, bufs, base, lane);
                    r!(d, lane) = v;
                }
            }
            Op::LoadLag(d, s, k) => {
                let base = series_base(ctx, *s, t.saturating_sub(*k));
                for lane in lo..hi {
                    let v = read(ctx, state, bufs, base, lane);
                    r!(d, lane) = v;
                }
            }
            Op::LoadAt(d, s, k) => {
                let base = series_base(ctx, *s, *k);
                for lane in lo..hi {
                    let v = read(ctx, state, bufs, base, lane);
                    r!(d, lane) = v;
                }
            }
            Op::LoadSeed(d, s) => match ctx.layout.place(*s) {
                Place::Series { seed, .. } => {
                    for lane in lo..hi {
                        r!(d, lane) = bufs.seeds[seed + lane];
                    }
                }
                Place::Hoisted { seed, .. } => {
                    let v = state.hoisted_seeds[seed];
                    for lane in lo..hi {
                        r!(d, lane) = v;
                    }
                }
                _ => {}
            },
            Op::LoadHoistedCur(d, s) => {
                let v = hoisted(ctx, state, *s, t);
                for lane in lo..hi {
                    r!(d, lane) = v;
                }
            }
            Op::LoadHoistedLag(d, s, k) => {
                let v = hoisted(ctx, state, *s, t.saturating_sub(*k));
                for lane in lo..hi {
                    r!(d, lane) = v;
                }
            }
            Op::LoadHoistedAt(d, s, k) => {
                let v = hoisted(ctx, state, *s, *k);
                for lane in lo..hi {
                    r!(d, lane) = v;
                }
            }
            Op::LoadTime(d, field) => {
                let v = ctx.clock.field(*field, t);
                for lane in lo..hi {
                    r!(d, lane) = v;
                }
            }
            Op::Add(d, a, b) => {
                for lane in lo..hi {
                    r!(d, lane) = r!(a, lane) + r!(b, lane);
                }
            }
            Op::Sub(d, a, b) => {
                for lane in lo..hi {
                    r!(d, lane) = r!(a, lane) - r!(b, lane);
                }
            }
            Op::Mul(d, a, b) => {
                for lane in lo..hi {
                    r!(d, lane) = r!(a, lane) * r!(b, lane);
                }
            }
            Op::Neg(d, a) => {
                for lane in lo..hi {
                    r!(d, lane) = -r!(a, lane);
                }
            }
            Op::Div {
                dst,
                lhs,
                rhs,
                mask,
                site,
            } => {
                for lane in lo..hi {
                    let (a, b) = (r!(lhs, lane), r!(rhs, lane));
                    let (v, trap) = ops::div(a, b);
                    r!(dst, lane) = v;
                    if let Some(kind) = trap {
                        if masked_in(bufs, cap, mask, lane)
                            && raise(
                                bufs,
                                mode,
                                lane,
                                kind,
                                Some(*site),
                                index,
                                &[("lhs", a), ("rhs", b)],
                            )
                        {
                            return false;
                        }
                    }
                }
            }
            Op::Pow {
                dst,
                lhs,
                rhs,
                mask,
                site,
            } => {
                for lane in lo..hi {
                    let (a, b) = (r!(lhs, lane), r!(rhs, lane));
                    let (v, trap) = ops::pow(a, b);
                    r!(dst, lane) = v;
                    if let Some(kind) = trap {
                        if masked_in(bufs, cap, mask, lane)
                            && raise(
                                bufs,
                                mode,
                                lane,
                                kind,
                                Some(*site),
                                index,
                                &[("lhs", a), ("rhs", b)],
                            )
                        {
                            return false;
                        }
                    }
                }
            }
            Op::Cmp(d, cmp, a, b) => {
                for lane in lo..hi {
                    r!(d, lane) = ops::cmp(*cmp, r!(a, lane), r!(b, lane));
                }
            }
            Op::And(d, a, b) => {
                for lane in lo..hi {
                    r!(d, lane) = f64::from(ops::truthy(r!(a, lane)) && ops::truthy(r!(b, lane)));
                }
            }
            Op::Or(d, a, b) => {
                for lane in lo..hi {
                    r!(d, lane) = f64::from(ops::truthy(r!(a, lane)) || ops::truthy(r!(b, lane)));
                }
            }
            Op::Not(d, a) => {
                for lane in lo..hi {
                    r!(d, lane) = f64::from(!ops::truthy(r!(a, lane)));
                }
            }
            Op::Select(d, c, a, b) => {
                for lane in lo..hi {
                    r!(d, lane) = if ops::truthy(r!(c, lane)) {
                        r!(a, lane)
                    } else {
                        r!(b, lane)
                    };
                }
            }
            Op::Call1(d, f, a) => {
                for lane in lo..hi {
                    let x = r!(a, lane);
                    let (v, trap) = ops::apply1(*f, x);
                    r!(d, lane) = v;
                    if let Some(kind) = trap {
                        if raise(bufs, mode, lane, kind, None, index, &[("arg", x)]) {
                            return false;
                        }
                    }
                }
            }
            Op::Call2(d, f, a, b) => {
                for lane in lo..hi {
                    let (x, y) = (r!(a, lane), r!(b, lane));
                    let (v, trap) = ops::apply2(*f, x, y);
                    r!(d, lane) = v;
                    if let Some(kind) = trap {
                        if raise(
                            bufs,
                            mode,
                            lane,
                            kind,
                            None,
                            index,
                            &[("arg0", x), ("arg1", y)],
                        ) {
                            return false;
                        }
                    }
                }
            }
            Op::CallN(d, f, args) => {
                let mut scratch = vec![0.0; args.len()];
                for lane in lo..hi {
                    for (i, a) in args.iter().enumerate() {
                        scratch[i] = r!(a, lane);
                    }
                    let (v, trap) = ops::applyn(*f, &scratch);
                    r!(d, lane) = v;
                    if let Some(kind) = trap {
                        let operands: Vec<(&str, f64)> =
                            scratch.iter().map(|&x| ("arg", x)).collect();
                        if raise(bufs, mode, lane, kind, None, index, &operands) {
                            return false;
                        }
                    }
                }
            }
            Op::Lookup {
                dst,
                table,
                keys,
                mask,
                site,
            } => {
                let binding = ctx.bindings[table.0 as usize];
                let compiled = &ctx.compiled[binding.table];
                let mut raw = vec![0.0; keys.len()];
                for lane in lo..hi {
                    for (i, k) in keys.iter().enumerate() {
                        raw[i] = r!(k, lane);
                    }
                    let args = key_args(compiled, &raw, ctx.dict);
                    let outcome = compiled.lookup_f64(&args, binding.value);
                    let (v, trap) = match outcome {
                        TableOutcome::Hit(v) | TableOutcome::Substituted(v) => (v, None),
                        TableOutcome::Trap(_) => (f64::NAN, Some(TrapKind::LookupMiss)),
                    };
                    r!(dst, lane) = v;
                    if let Some(kind) = trap {
                        let operands: Vec<(&str, f64)> = raw.iter().map(|&x| ("key", x)).collect();
                        if masked_in(bufs, cap, mask, lane)
                            && raise(bufs, mode, lane, kind, Some(*site), index, &operands)
                        {
                            return false;
                        }
                    }
                }
            }
            Op::Cum(d, s, acc) => {
                let base = series_base(ctx, *s, t);
                let acc_base = acc.0 as usize * cap;
                for lane in lo..hi {
                    let x = read(ctx, state, bufs, base, lane);
                    // The accumulator is the running total *including* t, and it
                    // is carried across periods in the arena, never recomputed.
                    let total = bufs.accs[acc_base + lane] + x;
                    if !replay {
                        bufs.accs[acc_base + lane] = total;
                    }
                    r!(d, lane) = total;
                }
            }
            Op::StoreCur(s, src) => match ctx.layout.place(*s) {
                Place::Series { .. } => {
                    let base = ctx.layout.series_at(*s, t).unwrap_or(0);
                    for lane in lo..hi {
                        bufs.series[base + lane] = r!(src, lane);
                    }
                }
                Place::Hoisted { offset, .. } => {
                    state.hoisted[offset + t as usize] = r!(src, lo);
                }
                _ => {}
            },
            Op::StoreSeed(s, src) => match ctx.layout.place(*s) {
                Place::Series { seed, .. } => {
                    for lane in lo..hi {
                        bufs.seeds[seed + lane] = r!(src, lane);
                    }
                }
                Place::Hoisted { seed, .. } => state.hoisted_seeds[seed] = r!(src, lo),
                _ => {}
            },
            Op::StoreHoisted(s, src) => {
                if let Place::Hoisted { offset, .. } = ctx.layout.place(*s) {
                    state.hoisted[offset + t as usize] = r!(src, lo);
                }
            }
            Op::StorePerMp(s, src) => {
                if let Place::PerMp { offset } = ctx.layout.place(*s) {
                    for lane in lo..hi {
                        bufs.permp[offset + lane] = r!(src, lane);
                    }
                    bufs.mark_written(offset + lo, hi - lo);
                }
            }
            Op::StoreScalar(s, src) => {
                if let Place::Scalar { index } = ctx.layout.place(*s) {
                    state.scalars[index] = r!(src, lo);
                }
            }
            Op::Reduce {
                dst,
                agg,
                series,
                pred,
            } => {
                for lane in lo..hi {
                    let v = crate::reduce::reduce(ctx, state, bufs, *agg, *series, *pred, lane);
                    r!(dst, lane) = v;
                }
            }
            Op::Npv {
                dst,
                value,
                disc,
                timing,
            } => {
                for lane in lo..hi {
                    let v = crate::reduce::npv(ctx, state, bufs, *value, *disc, *timing, lane);
                    r!(dst, lane) = v;
                }
            }
        }
    }
    true
}

/// Where a series' period-`t` row starts, or `None` for a hoisted slot.
fn series_base(ctx: &Ctx<'_>, slot: SlotId, t: u32) -> Result<usize, SlotId> {
    match ctx.layout.series_at(slot, t) {
        Some(base) => Ok(base),
        None => Err(slot),
    }
}

/// Read one lane of a series row, whether it is chunked or hoisted.
#[inline]
fn read(
    ctx: &Ctx<'_>,
    state: &RunState,
    bufs: &ChunkBuffers,
    base: Result<usize, SlotId>,
    lane: usize,
) -> f64 {
    match base {
        Ok(base) => bufs.series[base + lane],
        Err(slot) => match ctx.layout.place(slot) {
            Place::Hoisted { offset, .. } => state.hoisted[offset],
            Place::PerMp { offset } => bufs.permp[offset + lane],
            Place::Scalar { index } => state.scalars[index],
            Place::Series { .. } => 0.0,
        },
    }
}

/// One period of a hoisted series (no modelpoint axis).
#[inline]
pub(crate) fn hoisted(ctx: &Ctx<'_>, state: &RunState, slot: SlotId, t: u32) -> f64 {
    match ctx.layout.place(slot) {
        Place::Hoisted { offset, .. } => state.hoisted[offset + t as usize],
        _ => 0.0,
    }
}

/// Whether a masked trapping op is live on this lane. `None` — no enclosing
/// `If` — is always live (`01-ir.md` §2.6).
#[inline]
fn masked_in(bufs: &ChunkBuffers, cap: usize, mask: &Option<Reg>, lane: usize) -> bool {
    match mask {
        None => true,
        Some(m) => ops::truthy(bufs.regs[m.0 as usize * cap + lane]),
    }
}

/// Raise a trap. In lane mode this is one bit; in replay mode it captures the
/// operands and asks the caller to stop. Returns `true` when execution should
/// stop.
fn raise(
    bufs: &mut ChunkBuffers,
    mode: &mut Mode<'_>,
    lane: usize,
    kind: TrapKind,
    site: Option<SiteId>,
    op_index: usize,
    operands: &[(&str, f64)],
) -> bool {
    match mode {
        Mode::Lanes(_) => {
            bufs.traps.set(lane);
            false
        }
        Mode::Replay { hit, .. } => {
            if hit.is_none() {
                **hit = Some(TrapHit {
                    kind,
                    site,
                    op_index,
                    operands: operands
                        .iter()
                        .map(|(n, v)| ((*n).to_string(), *v))
                        .collect(),
                });
            }
            true
        }
    }
}

/// Build the typed key tuple a compiled table expects from raw lane values.
///
/// The lane carries an `f64` for every dtype — a `str`/`enum` key is a
/// dictionary code (§4.2) — so the key's compiled index decides how to read it.
pub(crate) fn key_args<'a>(
    table: &CompiledTable,
    raw: &[f64],
    dict: &'a Dictionary,
) -> Vec<KeyArg<'a>> {
    raw.iter()
        .enumerate()
        .map(|(i, &x)| match table.key_index(i).kind() {
            KeyIndexKind::Dictionary => KeyArg::Str(dict.decode(x)),
            KeyIndexKind::SortedFloat => KeyArg::Float(x),
            _ => KeyArg::Int(x as i64),
        })
        .collect()
}
