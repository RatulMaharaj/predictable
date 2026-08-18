//! The only clock the CLI has.
//!
//! A manifest carries `started_at` / `finished_at`, which `04-verify.md` §6.3 calls
//! out as the *only* non-deterministic fields it contains — they are deliberately
//! outside `manifest_digest`. Formatting them lives here, in one function, so no
//! command invents a second time format.

/// Now, as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn now_iso8601() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    iso8601(secs)
}

/// Unix seconds → `YYYY-MM-DDTHH:MM:SSZ`.
pub fn iso8601(unix_seconds: i64) -> String {
    let days = unix_seconds.div_euclid(86_400);
    let rem = unix_seconds.rem_euclid(86_400);
    let (y, m, d) = predictable_engine::dates::civil_from_days(days);
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Milliseconds since the process-wide monotonic start of a run.
pub fn elapsed_ms(start: std::time::Instant) -> u64 {
    start.elapsed().as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::iso8601;

    #[test]
    fn the_epoch_and_a_known_instant_format_exactly() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(1_786_857_262), "2026-08-16T05:14:22Z");
    }
}
