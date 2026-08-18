//! Comparing against — and regenerating — the committed goldens.
//!
//! A golden is compared byte for byte after `#` comment lines are dropped. Nothing here is
//! clever on purpose: the moment a harness starts normalising whitespace or rounding numbers to
//! make a comparison pass, it stops testing the thing it was built to test.

use std::path::{Path, PathBuf};

use crate::render::strip_comments;

/// `crates/predictable-determinism/golden`.
pub fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("golden")
}

/// True when `UPDATE_GOLDEN=1` is set — the deliberate, review-visible regeneration switch.
pub fn update_requested() -> bool {
    std::env::var("UPDATE_GOLDEN").is_ok_and(|v| v == "1")
}

/// Compare `actual` with `golden/<name>`, or write it when regenerating.
///
/// Returns `Err(report)` rather than panicking so a caller can collect every failing case and
/// report them together — one run of the harness should tell you everything that moved.
pub fn assert_golden(name: &str, actual: &str) -> Result<(), String> {
    let path = golden_dir().join(name);
    if update_requested() {
        std::fs::create_dir_all(golden_dir()).map_err(|e| e.to_string())?;
        std::fs::write(&path, actual).map_err(|e| e.to_string())?;
        return Ok(());
    }
    let expected = std::fs::read_to_string(&path).map_err(|_| {
        format!(
            "golden {name} is missing; run UPDATE_GOLDEN=1 cargo test -p predictable-determinism"
        )
    })?;
    let (want, got) = (strip_comments(&expected), strip_comments(actual));
    if want == got {
        return Ok(());
    }
    Err(format!(
        "{name} differs from its golden:\n{}",
        diff(&want, &got)
    ))
}

/// The first few differing lines, with context — enough to see whether a run moved by a ULP or
/// by a mile.
fn diff(want: &str, got: &str) -> String {
    let (w, g): (Vec<&str>, Vec<&str>) = (want.lines().collect(), got.lines().collect());
    let mut out = String::new();
    let mut shown = 0;
    for i in 0..w.len().max(g.len()) {
        let (a, b) = (w.get(i).copied(), g.get(i).copied());
        if a != b {
            out.push_str(&format!(
                "  line {}:\n    golden: {}\n    actual: {}\n",
                i + 1,
                a.unwrap_or("<missing>"),
                b.unwrap_or("<missing>")
            ));
            shown += 1;
            if shown == 8 {
                out.push_str("  … (truncated)\n");
                break;
            }
        }
    }
    out
}
