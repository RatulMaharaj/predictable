//! The tape digest.
//!
//! `order_digest` (`03-engine.md` §3.2) makes a change in evaluation *order* a
//! reviewable event. The tape digest does the same one level down: it hashes the
//! canonical text of every op in every tape, so a change in lowering — a new
//! mask, a different peel, a register renumbering — is visible as a digest
//! change in a golden test rather than as a silent difference in generated code.
//!
//! It hashes the *text*, not the serde encoding, for the same reason
//! `model_digest` does: text is the form a human can diff when the digests
//! disagree.

use sha2::{Digest, Sha256};

use crate::TapeProgram;

/// `sha256` over the canonical text of the whole program, plus the table list.
pub fn tape_digest(prog: &TapeProgram) -> String {
    let mut h = Sha256::new();
    h.update(format!("periods={}\npeel={}\n", prog.periods, prog.peel));
    for t in &prog.tables {
        h.update(format!("table={t}\n"));
    }
    h.update(prog.text());
    format!("{:x}", h.finalize())
}
