//! Proleptic-Gregorian date arithmetic on days-since-1970, with no dependency.
//!
//! Dates live in `f64` lanes as an integral day count, which is exact for every
//! date the IR can express (`|days| < 2^53`). The conversions are Howard
//! Hinnant's `days_from_civil` / `civil_from_days`, chosen because they are
//! branch-light, exact, and identical on every target — a date library that
//! consulted the host timezone would break determinism (§9).

/// Days since 1970-01-01 for a proleptic-Gregorian `(y, m, d)`.
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y - i64::from(m <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The inverse: `(year, month, day)`.
pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    (y + i64::from(m <= 2), m, d)
}

/// Parse an ISO-8601 `YYYY-MM-DD` into days since epoch. The IR guarantees the
/// canonical form, so anything else is a bug upstream and yields `None`.
pub fn parse_iso(text: &str) -> Option<i64> {
    let b = text.as_bytes();
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let y: i64 = text[0..4].parse().ok()?;
    let m: i64 = text[5..7].parse().ok()?;
    let d: i64 = text[8..10].parse().ok()?;
    Some(days_from_civil(y, m, d))
}

/// `add_months(d, n)`, clamping the day to the target month's length — the
/// convention every actuarial system uses for a 31st into February.
pub fn add_months(days: i64, n: i64) -> i64 {
    let (y, m, d) = civil_from_days(days);
    let total = (y * 12 + (m - 1)) + n;
    let (ny, nm) = (total.div_euclid(12), total.rem_euclid(12) + 1);
    let nd = d.min(days_in_month(ny, nm));
    days_from_civil(ny, nm, nd)
}

/// Whole months from `a` to `b`, truncated toward zero — the day-of-month tail
/// is dropped, matching `months_between` in §2.8.
pub fn months_between(a: i64, b: i64) -> i64 {
    let (ay, am, ad) = civil_from_days(a);
    let (by, bm, bd) = civil_from_days(b);
    let mut months = (by * 12 + bm) - (ay * 12 + am);
    if months > 0 && bd < ad {
        months -= 1;
    } else if months < 0 && bd > ad {
        months += 1;
    }
    months
}

/// Length of a month, leap years included.
pub fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if is_leap(y) => 29,
        _ => 28,
    }
}

pub fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}
