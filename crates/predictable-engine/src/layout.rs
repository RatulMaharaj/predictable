//! Storage layout: slot → offset, decided once per `(Plan, chunk size)`.
//!
//! `03-engine.md` §4.2 fixes the shape: `series_buf[slot][t_ring][c]`, with `c`
//! — the modelpoint within the chunk — innermost and contiguous. Every offset a
//! kernel op needs is therefore one array read ([`Layout::place`]), never a map
//! lookup, and the whole chunk is one flat allocation ([`crate::ChunkBuffers`]).
//!
//! Two consequences worth naming, because they are the reason for the layout:
//!
//! * the innermost loop is over lanes, so element-wise arithmetic vectorises
//!   without reassociating anything — a lane never sees another lane's value;
//! * retention (§4.3) is a *layout* decision, not a run-time one: a `Ring(k+1)`
//!   slot occupies `next_power_of_two(k+1)` periods and is indexed with a mask,
//!   a `Full` slot occupies `T+1`.

use std::collections::BTreeSet;

use predictable_plan::{Plan, Retention, SlotId, Space};

/// Where one slot's storage lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// Per-run `f64`, index into [`crate::RunShared::scalars`].
    Scalar { index: usize },
    /// One lane per modelpoint: `permp[offset + c]`.
    PerMp { offset: usize },
    /// `series[offset + (t & mask) * chunk + c]`, or `t * chunk + c` when full.
    Series {
        offset: usize,
        /// Number of retained periods; a power of two unless `full`.
        periods: usize,
        mask: usize,
        full: bool,
        /// Lane-strided slot in the seed buffer for this slot's `init`.
        seed: usize,
    },
    /// Loop-invariant series: `hoisted[offset + t]`, no modelpoint axis (§3.4).
    Hoisted { offset: usize, seed: usize },
}

/// Slot → storage, plus the sizes of every buffer a chunk needs.
#[derive(Debug, Clone)]
pub struct Layout {
    places: Vec<Place>,
    /// Modelpoints per chunk (`C`).
    pub chunk: usize,
    /// `T`; the projection runs `t = 0..=periods`.
    pub periods: u32,
    /// `f64`s in the per-chunk series buffer.
    pub series_len: usize,
    /// `f64`s in the per-chunk `PerMP` buffer.
    pub permp_len: usize,
    /// `f64`s in the per-chunk seed buffer.
    pub seed_len: usize,
    /// `f64`s in the run-level hoisted buffer (`(T+1)` per hoisted slot).
    pub hoisted_len: usize,
    /// Run-level hoisted seeds, one per hoisted slot.
    pub hoisted_seed_len: usize,
    /// `Scalar` slots.
    pub scalar_len: usize,
    /// Widest register frame across every tape.
    pub n_regs: u16,
    /// Widest `cum()` accumulator frame across every tape.
    pub n_accs: u16,
}

impl Layout {
    /// Lay out `plan` for chunks of `chunk` modelpoints.
    ///
    /// `n_regs` and `n_accs` come from the tape program, because the register
    /// file is a property of the lowering, not of the plan.
    pub fn new(plan: &Plan, chunk: usize, n_regs: u16, n_accs: u16) -> Layout {
        Layout::with_full(plan, chunk, n_regs, n_accs, &BTreeSet::new())
    }

    /// Lay out `plan`, forcing `Full` retention on `force_full`.
    ///
    /// The substage leveller (§5.3) needs this: a level-0 series read by a
    /// level-1 slot is read *after* level 0's `t` loop has finished, so a ring
    /// buffer would have wrapped its history away. Retention is a layout
    /// decision, so the repair belongs here rather than in a second opinion
    /// about the plan. Slots not named keep the planner's choice.
    pub fn with_full(
        plan: &Plan,
        chunk: usize,
        n_regs: u16,
        n_accs: u16,
        force_full: &BTreeSet<SlotId>,
    ) -> Layout {
        let mut places = Vec::with_capacity(plan.slot_refs.len());
        let (mut series_len, mut permp_len, mut seed_len) = (0usize, 0usize, 0usize);
        let (mut hoisted_len, mut hoisted_seeds) = (0usize, 0usize);
        let t_len = plan.periods as usize + 1;

        for id in 0..plan.slot_refs.len() {
            let r = plan.slot_refs[id];
            let place = match r.space {
                Space::Scalar => Place::Scalar {
                    index: r.index as usize,
                },
                Space::PerMp => {
                    let offset = permp_len;
                    permp_len += chunk;
                    Place::PerMp { offset }
                }
                Space::Series => {
                    let slot = &plan.series[r.index as usize];
                    if slot.hoistable {
                        let offset = hoisted_len;
                        hoisted_len += t_len;
                        let seed = hoisted_seeds;
                        hoisted_seeds += 1;
                        Place::Hoisted { offset, seed }
                    } else {
                        let full =
                            slot.retention.is_full() || force_full.contains(&SlotId(id as u32));
                        let periods = match slot.retention {
                            _ if full => t_len,
                            Retention::Full => t_len,
                            Retention::Ring { len } => (len as usize).max(1).next_power_of_two(),
                        };
                        let offset = series_len;
                        series_len += periods * chunk;
                        let seed = seed_len;
                        seed_len += chunk;
                        Place::Series {
                            offset,
                            periods,
                            mask: if full { 0 } else { periods - 1 },
                            full,
                            seed,
                        }
                    }
                }
            };
            places.push(place);
        }

        Layout {
            places,
            chunk,
            periods: plan.periods,
            series_len,
            permp_len,
            seed_len,
            hoisted_len,
            hoisted_seed_len: hoisted_seeds,
            scalar_len: plan.scalars.len(),
            n_regs,
            n_accs,
        }
    }

    /// One array read: the storage decision for `slot`.
    pub fn place(&self, slot: SlotId) -> Place {
        self.places[slot.index()]
    }

    /// Offset of `slot` at period `t` within the chunk series buffer.
    ///
    /// `Ring` slots wrap with a mask (`t & (len - 1)`) — no modulo, no branch,
    /// which is why ring lengths are rounded up to a power of two (§4.3).
    pub fn series_at(&self, slot: SlotId, t: u32) -> Option<usize> {
        match self.place(slot) {
            Place::Series {
                offset, mask, full, ..
            } => {
                let row = if full { t as usize } else { t as usize & mask };
                Some(offset + row * self.chunk)
            }
            _ => None,
        }
    }

    /// Bytes one `ChunkBuffers` occupies — what the CLI prints before it
    /// allocates, and what `--retain-all` multiplies (§4.3).
    pub fn chunk_bytes(&self) -> u64 {
        (self.series_len + self.permp_len + self.seed_len) as u64 * 8
            + self.chunk as u64 * self.n_regs as u64 * 8
            + self.chunk as u64 * self.n_accs as u64 * 8
    }
}
