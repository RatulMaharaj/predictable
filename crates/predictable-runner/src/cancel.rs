//! The cancel flag (`03-engine.md` §10).
//!
//! Cancellation is *cooperative and chunk-granular*: a worker checks the flag before it starts a
//! chunk, never inside the `t` loop. That keeps the kernel free of any notion of cancellation —
//! it stays a pure function — and it makes the outcome honest: a cancelled run writes a manifest
//! with `"outcome": "cancelled"` and `execution.modelpoints_projected` short of the file's row
//! count (`01-ir.md` §9.3.1), rather than a truncated result set that looks complete.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A shared, cloneable "stop after the current chunk" flag.
///
/// Cloning shares the flag; setting it from any thread (or from Python, across
/// `Python::allow_threads` — `03-engine.md` §8.2) is visible to every worker.
#[derive(Debug, Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    /// A flag that has not been set.
    pub fn new() -> CancelFlag {
        CancelFlag::default()
    }

    /// Request cancellation. Idempotent, and safe from any thread or a signal handler.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// True once [`CancelFlag::cancel`] has been called.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clone_shares_the_flag() {
        let a = CancelFlag::new();
        let b = a.clone();
        assert!(!b.is_cancelled());
        a.cancel();
        assert!(b.is_cancelled());
        // Idempotent.
        b.cancel();
        assert!(a.is_cancelled());
    }
}
