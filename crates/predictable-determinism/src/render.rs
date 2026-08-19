//! Rendering a run as reviewable, byte-comparable text.
//!
//! Two rules govern the format:
//!
//! 1. **Every float is its bit pattern.** `0x3fb999999999999a` is the assertion; the decimal
//!    beside it is for the human reading the diff. A decimal-only golden would tolerate a
//!    last-ULP divergence between glibc's `pow` and macOS's, which is precisely the class of bug
//!    §7 exists to catch.
//! 2. **Nothing non-deterministic is compared.** The host target is printed as a `#` comment so
//!    a reviewer knows where a golden came from, and comment lines are stripped before
//!    comparison ([`strip_comments`]).

use std::fmt::Write as _;

use crate::corpus::{Loaded, RunOutput};

/// The IEEE-754 bit pattern of a float, as it is stored.
///
/// NaN payloads survive this, which is the point: two engines that produce different NaNs
/// produce different bytes in `results.parquet`, so the harness must not normalise them away.
pub fn bits(v: f64) -> String {
    format!("0x{:016x}", v.to_bits())
}

/// The target triple this build is for — provenance in a golden header, never an assertion.
pub fn host_target() -> String {
    format!(
        "{}-{}-{}",
        std::env::consts::ARCH,
        std::env::consts::FAMILY,
        std::env::consts::OS
    )
}

/// Drop `#` comment lines so provenance never participates in the comparison.
pub fn strip_comments(text: &str) -> String {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

/// One float, rendered as bits plus a shortest-round-trip decimal.
fn value(v: f64) -> String {
    format!("{} {:?}", bits(v), v)
}

/// The full golden text for a case: digest chain, plan order, then every emitted value.
pub fn render_golden(loaded: &Loaded, out: &RunOutput) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "# predictable determinism golden");
    let _ = writeln!(s, "# generated on: {}", host_target());
    let _ = writeln!(
        s,
        "# regenerate with: UPDATE_GOLDEN=1 cargo test -p predictable-determinism"
    );
    let _ = writeln!(s, "case {}", loaded.name);
    let _ = writeln!(s, "modelpoints {}", loaded.modelpoints.0);
    let _ = writeln!(
        s,
        "modelpoint_digest {}",
        predictable_fmt::digest::digest_bytes(loaded.modelpoints.1.as_bytes())
    );
    let _ = writeln!(s, "model_digest {}", loaded.model_digest);
    let _ = writeln!(s, "order_digest {}", loaded.plan.order_digest);
    let _ = writeln!(s, "plan_digest {}", loaded.plan.digest);
    let _ = writeln!(s, "tape_digest {}", loaded.tape_digest);
    let _ = writeln!(s, "periods {}", loaded.plan.periods);
    let _ = writeln!(s, "order {}", loaded.plan.order_names().join(" "));

    for table in &loaded.tables {
        let _ = writeln!(
            s,
            "table {} rows={} index={} drifted={} digest={}",
            table.name(),
            table.rows(),
            table.stats().index,
            table.drifted,
            table.digest
        );
    }
    for (name, v) in &loaded.assumptions {
        let _ = writeln!(s, "assumption {name} {}", value(*v));
    }

    let p = &out.projection;
    let _ = writeln!(s, "outcome {:?} exit={}", p.outcome, p.exit_code());
    let _ = writeln!(
        s,
        "projected {} trapped {} traps {}",
        p.modelpoints_projected, p.modelpoints_trapped, p.trap_total
    );

    // A trap is part of the result, not an aside: two engines that trap differently have
    // produced different runs even when every surviving number agrees.
    for trap in &p.traps {
        let _ = writeln!(s, "trap {trap:?}");
    }

    // Results in `(chunk_idx, offset)` order — the order the writer emits, so the golden is the
    // result set's own order and not a re-sorted view of it.
    for chunk in &p.chunks {
        for (lane, key) in chunk.keys.iter().enumerate() {
            for column in &chunk.columns {
                let lane_values = column.lane(lane);
                if column.stride == 1 {
                    let _ = writeln!(s, "{key} {} . {}", column.name, value(lane_values[0]));
                } else {
                    for (t, v) in lane_values.iter().enumerate() {
                        let _ = writeln!(s, "{key} {} {t} {}", column.name, value(*v));
                    }
                }
            }
        }
        for dropped in &chunk.dropped {
            let _ = writeln!(s, "{dropped} <dropped>");
        }
    }
    for row in &p.aggregates {
        let _ = writeln!(s, "aggregate {row:?}");
    }
    s
}
