//! Linear-scan register allocation (`03-engine.md` §3.3).
//!
//! Lowering hands out one virtual register per value. That is the right form to
//! reason about — it is SSA, so a value's identity never changes — but it is the
//! wrong form to *run*: the kernel allocates `C × n_regs` scratch floats per
//! worker, so 400 virtual registers would mean a 3 MB frame per thread instead
//! of a cache-resident one.
//!
//! The tape is straight-line (`Select` replaced the branch, §3.3), so live
//! ranges are plain intervals and the textbook linear scan is exact rather than
//! approximate: a virtual register is live from its definition to its last use,
//! and two registers may share a physical slot iff their intervals are
//! disjoint. There is no spilling — a tape that needs more than 65,536 live
//! values is rejected at lowering (`LowerError::TooManyRegisters`) rather than
//! quietly generating memory traffic.
//!
//! Two properties are deliberate and tested:
//!
//! * **Uses are freed before the definition is allocated.** `r0 = add r0, r1` is
//!   allowed, because every op is elementwise over lanes and reads a lane before
//!   it writes it. This is what keeps a long chain of arithmetic down to two or
//!   three physical registers.
//! * **The free list is a min-heap, not a stack.** The lowest free register is
//!   always taken, so the allocation is a function of the tape alone — same
//!   plan, same numbering, on any machine, which `order_digest`'s sibling
//!   [`crate::digest`] then hashes.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

use crate::op::{Op, RegRole};

/// Renumber `ops` in place onto a dense physical register file, returning the
/// number of registers used.
pub fn allocate(ops: &mut [Op]) -> u16 {
    // Last use per virtual register.
    let mut last_use: BTreeMap<u16, usize> = BTreeMap::new();
    for (i, op) in ops.iter().enumerate() {
        op.for_each_reg(|r, role| {
            if role == RegRole::Use {
                last_use.insert(r.0, i);
            }
        });
    }

    let mut map: BTreeMap<u16, u16> = BTreeMap::new();
    let mut free: BinaryHeap<Reverse<u16>> = BinaryHeap::new();
    let mut next: u32 = 0;
    let mut high_water: u32 = 0;

    // Indexed rather than iterated: each op is visited twice, once for its uses
    // and once for its definition, and the two passes must see the same
    // element.
    #[allow(clippy::needless_range_loop)]
    for i in 0..ops.len() {
        // 1. Rewrite uses, and note which physical registers die here.
        let mut dying: Vec<u16> = Vec::new();
        ops[i].for_each_reg_mut(|r, role| {
            if role != RegRole::Use {
                return;
            }
            let virt = r.0;
            let phys = map.get(&virt).copied().unwrap_or(virt);
            r.0 = phys;
            if last_use.get(&virt) == Some(&i) {
                dying.push(phys);
            }
        });
        dying.sort_unstable();
        dying.dedup();
        for phys in dying {
            free.push(Reverse(phys));
        }

        // 2. Allocate the definition — after the frees, so `r0 = add r0, r1`
        //    is reachable.
        let mut defined: Option<(u16, u16)> = None;
        ops[i].for_each_reg_mut(|r, role| {
            if role != RegRole::Def {
                return;
            }
            let virt = r.0;
            let phys = match free.pop() {
                Some(Reverse(p)) => p,
                None => {
                    let p = next as u16;
                    next += 1;
                    high_water = high_water.max(next);
                    p
                }
            };
            r.0 = phys;
            defined = Some((virt, phys));
        });
        if let Some((virt, phys)) = defined {
            map.insert(virt, phys);
            // A value nothing reads is dead on arrival: return it at once so a
            // store-only tape does not grow a register per op.
            if !last_use.contains_key(&virt) {
                free.push(Reverse(phys));
            }
        }
    }

    high_water.min(u16::MAX as u32) as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::op::{ConstId, Reg};
    use predictable_plan::SlotId;

    fn c(dst: u16, k: u32) -> Op {
        Op::ConstF(Reg(dst), ConstId(k))
    }

    #[test]
    fn chain_reuses_two_registers() {
        // v0 = 1; v1 = 2; v2 = v0 + v1; v3 = 3; v4 = v2 + v3; store v4
        let mut ops = vec![
            c(0, 0),
            c(1, 1),
            Op::Add(Reg(2), Reg(0), Reg(1)),
            c(3, 2),
            Op::Add(Reg(4), Reg(2), Reg(3)),
            Op::StoreCur(SlotId(7), Reg(4)),
        ];
        let n = allocate(&mut ops);
        assert_eq!(n, 2, "a left-leaning chain needs exactly two registers");
        assert_eq!(ops[2], Op::Add(Reg(0), Reg(0), Reg(1)));
        assert_eq!(ops[5], Op::StoreCur(SlotId(7), Reg(0)));
    }

    #[test]
    fn overlapping_live_ranges_get_distinct_registers() {
        // v0 lives across the whole tape and must not be clobbered.
        let mut ops = vec![
            c(0, 0),
            c(1, 1),
            c(2, 2),
            Op::Add(Reg(3), Reg(1), Reg(2)),
            Op::Add(Reg(4), Reg(0), Reg(3)),
            Op::StoreCur(SlotId(1), Reg(4)),
        ];
        let n = allocate(&mut ops);
        assert_eq!(n, 3);
        // r0 survives untouched to its use at op 4: nothing defined between
        // op 0 and op 4 was allowed to take it.
        assert!(ops[1..4].iter().all(|op| op.def() != Some(Reg(0))));
        assert_eq!(ops[4], Op::Add(Reg(0), Reg(0), Reg(1)));
    }

    #[test]
    fn dead_definitions_are_recycled_immediately() {
        let mut ops = vec![c(0, 0), c(1, 1), c(2, 2)];
        assert_eq!(allocate(&mut ops), 1);
    }
}
