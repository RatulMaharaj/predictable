//! The tape itself: a flat `Vec<Op>` plus the side tables it indexes into.
//!
//! A tape is built once per group (prologue, hoisted, stage 1, stage 2) and
//! never mutated afterwards. Within a tape, each slot's expression occupies one
//! contiguous [`TapeRange`] — which is what lets `explain()` replay a single
//! component, and what lets the substage leveller of §5.3 run a subset of the
//! stage-1 slots without rebuilding anything.

use std::collections::BTreeMap;
use std::fmt;

use predictable_plan::SlotId;
use serde::{Deserialize, Serialize};

use crate::op::{AccId, ConstId, Op, Reg, SiteId};

/// Where a trap or an `explain()` node is reported.
///
/// `03-engine.md` §3.3 calls this a `SpanId`. Spans live in `predictable-syntax`
/// and do not survive into the IR, so the tape carries the durable identity
/// instead: the owning slot plus the `ExprPath` of `01-ir.md` §3.0.1 (`Q9`),
/// which is derived from the canonical expression and is stable across
/// reformatting. Turning one back into a source span is a lookup in the IR's
/// `meta.origin_span`, which is where that mapping belongs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Site {
    pub slot: SlotId,
    /// Dotted path from `expr` or `init`, e.g. `expr.rhs.arg1`.
    pub path: String,
    /// Set on ops the peeling pass resolved below the origin — the
    /// `pre_origin_default` provenance note of §3.3.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pre_origin_default: bool,
}

/// The half-open op range that computes one slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TapeRange {
    pub slot: SlotId,
    pub start: u32,
    pub end: u32,
}

impl TapeRange {
    pub fn len(&self) -> u32 {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.end == self.start
    }
}

/// A straight-line program over lane registers.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Tape {
    pub ops: Vec<Op>,
    /// Physical register count after allocation — the width of the scratch
    /// frame the kernel allocates (`C × n_regs`).
    pub n_regs: u16,
    pub const_f: Vec<f64>,
    pub const_i: Vec<i64>,
    /// `str` / `date` / `enum` literals in canonical text form; the kernel
    /// dictionary-encodes them at load (`03-engine.md` §4.2).
    pub const_s: Vec<String>,
    pub sites: Vec<Site>,
    /// One entry per slot this tape computes, in tape order.
    pub ranges: Vec<TapeRange>,
    /// Number of `cum()` accumulators the kernel must carry across `t`.
    pub accumulators: u16,
}

impl Tape {
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ops.len()
    }

    /// The op range that computes `slot`, if this tape computes it.
    pub fn range_of(&self, slot: SlotId) -> Option<TapeRange> {
        self.ranges.iter().copied().find(|r| r.slot == slot)
    }

    /// The canonical text form: one op per line, `NN | op`. Golden tests and the
    /// docs page assert on this, and [`crate::digest`] hashes it.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for (i, op) in self.ops.iter().enumerate() {
            out.push_str(&format!("{i:>3} | {op}\n"));
        }
        out
    }
}

impl fmt::Display for Tape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text())
    }
}

/// Builds a tape: interns constants and sites, hands out virtual registers.
///
/// Registers handed out here are *virtual* — one per value, never reused — and
/// are renumbered onto a small physical file by [`crate::regalloc`] once the
/// whole tape is known. Doing it in that order is what makes the allocation a
/// true linear scan over live ranges rather than a stack discipline that would
/// leak registers across common subexpressions.
#[derive(Debug, Default)]
pub struct TapeBuilder {
    pub(crate) ops: Vec<Op>,
    next_reg: u32,
    const_f: Vec<f64>,
    const_f_index: BTreeMap<u64, u32>,
    const_i: Vec<i64>,
    const_i_index: BTreeMap<i64, u32>,
    const_s: Vec<String>,
    const_s_index: BTreeMap<String, u32>,
    sites: Vec<Site>,
    site_index: BTreeMap<Site, u32>,
    ranges: Vec<TapeRange>,
    accumulators: u16,
}

impl TapeBuilder {
    pub fn new() -> TapeBuilder {
        TapeBuilder::default()
    }

    /// A fresh virtual register.
    pub fn reg(&mut self) -> Result<Reg, crate::LowerError> {
        if self.next_reg > u16::MAX as u32 {
            return Err(crate::LowerError::TooManyRegisters);
        }
        let r = Reg(self.next_reg as u16);
        self.next_reg += 1;
        Ok(r)
    }

    pub fn emit(&mut self, op: Op) {
        self.ops.push(op);
    }

    /// Intern an `f64` by bit pattern, so `-0.0` and `0.0` stay distinct and
    /// `NaN` interns once. Value equality would merge them and change bits.
    pub fn const_f(&mut self, x: f64) -> ConstId {
        let key = x.to_bits();
        if let Some(&id) = self.const_f_index.get(&key) {
            return ConstId(id);
        }
        let id = self.const_f.len() as u32;
        self.const_f.push(x);
        self.const_f_index.insert(key, id);
        ConstId(id)
    }

    pub fn const_i(&mut self, x: i64) -> ConstId {
        if let Some(&id) = self.const_i_index.get(&x) {
            return ConstId(id);
        }
        let id = self.const_i.len() as u32;
        self.const_i.push(x);
        self.const_i_index.insert(x, id);
        ConstId(id)
    }

    pub fn const_s(&mut self, s: &str) -> ConstId {
        if let Some(&id) = self.const_s_index.get(s) {
            return ConstId(id);
        }
        let id = self.const_s.len() as u32;
        self.const_s.push(s.to_string());
        self.const_s_index.insert(s.to_string(), id);
        ConstId(id)
    }

    pub fn site(&mut self, site: Site) -> SiteId {
        if let Some(&id) = self.site_index.get(&site) {
            return SiteId(id);
        }
        let id = self.sites.len() as u32;
        self.sites.push(site.clone());
        self.site_index.insert(site, id);
        SiteId(id)
    }

    pub fn accumulator(&mut self) -> AccId {
        let id = AccId(self.accumulators);
        self.accumulators += 1;
        id
    }

    pub fn mark(&self) -> u32 {
        self.ops.len() as u32
    }

    /// Record that the ops emitted since `start` compute `slot`.
    pub fn range(&mut self, slot: SlotId, start: u32) {
        let end = self.ops.len() as u32;
        if end > start {
            self.ranges.push(TapeRange { slot, start, end });
        }
    }

    /// Finish: run the register allocator and freeze.
    pub fn finish(self) -> Tape {
        let mut ops = self.ops;
        let n_regs = crate::regalloc::allocate(&mut ops);
        Tape {
            ops,
            n_regs,
            const_f: self.const_f,
            const_i: self.const_i,
            const_s: self.const_s,
            sites: self.sites,
            ranges: self.ranges,
            accumulators: self.accumulators,
        }
    }
}
