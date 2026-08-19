//! The per-worker arena (`03-engine.md` §4.4).
//!
//! One [`ChunkBuffers`] is allocated per worker thread at run start and reused
//! for every chunk. [`ChunkBuffers::reset`] does **not** zero memory: every slot
//! is written before it is read, guaranteed by the topological order, and that
//! invariant is asserted in debug builds by a per-lane initialisation bitmap
//! rather than paid for in release. Zero allocation in the projection loop is a
//! determinism property as much as a speed one — nothing here depends on the
//! allocator's behaviour.

use crate::layout::Layout;
use crate::traps::TrapFlags;

/// The scratch an in-flight chunk owns.
#[derive(Debug)]
pub struct ChunkBuffers {
    /// `series[slot_offset + t_row * chunk + c]` (§4.2).
    pub series: Vec<f64>,
    /// One lane per modelpoint per `PerMP` slot.
    pub permp: Vec<f64>,
    /// `init` seeds, one lane per modelpoint per series slot.
    pub seeds: Vec<f64>,
    /// `chunk × n_regs` register frame, reused across `t` and across chunks.
    pub regs: Vec<f64>,
    /// `chunk × n_accs` running totals for `cum()`, carried across `t`.
    pub accs: Vec<f64>,
    /// Per-lane trap state for the period in flight (§5.5).
    pub traps: TrapFlags,
    /// Lanes that trapped anywhere in this chunk, and are therefore abandoned.
    pub dead: TrapFlags,
    /// Modelpoints actually present in the chunk in flight (`≤ chunk`).
    pub lanes: usize,
    chunk: usize,
    /// Debug-only initialisation bitmap over the `PerMP` buffer (§4.4). Series
    /// rows are excluded: a ring row is legitimately overwritten every
    /// `ring_len` periods, so "written since reset" is not the invariant there.
    #[cfg(debug_assertions)]
    written: Vec<bool>,
}

impl ChunkBuffers {
    /// Allocate the arena for one worker. This is the only allocation a run
    /// performs after planning.
    pub fn new(layout: &Layout) -> ChunkBuffers {
        let chunk = layout.chunk;
        ChunkBuffers {
            series: vec![0.0; layout.series_len],
            permp: vec![0.0; layout.permp_len],
            seeds: vec![0.0; layout.seed_len],
            regs: vec![0.0; chunk * layout.n_regs as usize],
            accs: vec![0.0; chunk * layout.n_accs as usize],
            traps: TrapFlags::new(chunk),
            dead: TrapFlags::new(chunk),
            lanes: 0,
            chunk,
            #[cfg(debug_assertions)]
            written: vec![false; layout.permp_len],
        }
    }

    /// Modelpoint capacity — `C`.
    pub fn capacity(&self) -> usize {
        self.chunk
    }

    /// Ready the arena for the next chunk. Memory is deliberately not zeroed;
    /// only the bookkeeping is cleared.
    pub fn reset(&mut self, lanes: usize) {
        debug_assert!(
            lanes <= self.chunk,
            "chunk overflow: {lanes} > {}",
            self.chunk
        );
        self.lanes = lanes;
        self.traps.clear();
        self.dead.clear();
        for a in self.accs.iter_mut() {
            *a = 0.0;
        }
        #[cfg(debug_assertions)]
        for w in self.written.iter_mut() {
            *w = false;
        }
    }

    /// Zero the `cum()` accumulators without disturbing anything else.
    ///
    /// The substage leveller (§5.3) runs the `t` loop once per level, and an
    /// accumulator belongs to the level whose loop drives it: level 1 must
    /// start its running totals at zero exactly as level 0 did.
    pub fn reset_accs(&mut self) {
        for a in self.accs.iter_mut() {
            *a = 0.0;
        }
    }

    /// Debug-only: record that `permp[base .. base+lanes)` was written.
    #[inline]
    pub(crate) fn mark_written(&mut self, _base: usize, _lanes: usize) {
        #[cfg(debug_assertions)]
        for i in 0.._lanes {
            if let Some(w) = self.written.get_mut(_base + i) {
                *w = true;
            }
        }
    }

    /// Debug-only: was this `PerMP` lane written since [`ChunkBuffers::reset`]?
    #[inline]
    pub(crate) fn is_written(&self, _base: usize) -> bool {
        #[cfg(debug_assertions)]
        let written = self.written.get(_base).copied().unwrap_or(true);
        #[cfg(not(debug_assertions))]
        let written = true;
        written
    }
}
