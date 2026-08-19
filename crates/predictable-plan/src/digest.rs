//! `order_digest` and `PlanDigest` (`03-engine.md` §3.1, §3.2).
//!
//! A change in topological order is a diffable, reviewable event, not an
//! accident of a `HashMap`. The digest is what makes it visible: it is checked
//! in the golden-corpus tests, so a reordering shows up as a one-line diff in a
//! pull request rather than as a mysterious change in results three weeks later.
//!
//! Both digests are `sha256` over an unambiguous byte encoding — every variable
//! part is length-prefixed or newline-terminated, so no two different plans can
//! hash the same bytes by concatenation accident.

use sha2::{Digest, Sha256};

use crate::slots::SlotId;

fn hex(bytes: impl AsRef<[u8]>) -> String {
    let digest = Sha256::digest(bytes.as_ref());
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// `sha256` of the slot ids in evaluation order, one tape at a time.
///
/// Tapes are separated by their name so that moving a slot from `stage1` to
/// `hoisted` — which is a real semantic change in where work happens — changes
/// the digest, even though the concatenated id sequence would not.
pub fn order_digest(tapes: &[(&str, &[SlotId])]) -> String {
    let mut buf = Vec::new();
    for (name, slots) in tapes {
        buf.extend_from_slice(name.as_bytes());
        buf.push(b'\n');
        buf.extend_from_slice(&(slots.len() as u32).to_le_bytes());
        for slot in *slots {
            buf.extend_from_slice(&slot.0.to_le_bytes());
        }
    }
    hex(buf)
}

/// `sha256(program_digest ‖ run_config ‖ engine_semver_major)`.
///
/// `run_config` is rendered by the caller as the already-canonical key=value
/// lines of the options that can change a plan; `order_digest` is folded in too,
/// so a plan digest pins the order it was built with.
pub fn plan_digest(program_digest: &str, run_config: &str, order_digest: &str) -> String {
    let engine_major = env!("CARGO_PKG_VERSION")
        .split('.')
        .next()
        .unwrap_or("0")
        .to_string();
    let mut buf = String::new();
    buf.push_str("program=");
    buf.push_str(program_digest);
    buf.push('\n');
    buf.push_str(run_config);
    if !run_config.ends_with('\n') {
        buf.push('\n');
    }
    buf.push_str("order=");
    buf.push_str(order_digest);
    buf.push('\n');
    buf.push_str("engine_major=");
    buf.push_str(&engine_major);
    buf.push('\n');
    hex(buf)
}
