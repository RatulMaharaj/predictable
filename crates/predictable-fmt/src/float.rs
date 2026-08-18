//! Rule 4 of `01-ir.md` §4.1: floats round-trip via shortest representation.
//!
//! The shortest representation is Ryū's, via the `ryu` crate. Ryū always emits a
//! fractional part or an exponent, so a float never loses its float-ness on the
//! way through `fmt` (`45.0` stays `45.0`, never `45`), and it picks exponent
//! notation only where the plain form would be longer (`1e-8`, not
//! `0.00000001`; `1000000.0`, not `1e6`).

/// Canonical text of an `f64`.
///
/// Non-finite values cannot occur in a `.pir` file — the grammar has no spelling
/// for them — but the function is total, and prints them the way Rust does so
/// that a bug is visible rather than silent.
pub fn format_f64(v: f64) -> String {
    if !v.is_finite() {
        return format!("{v}");
    }
    let mut buf = ryu::Buffer::new();
    buf.format_finite(v).to_string()
}

#[cfg(test)]
mod tests {
    use super::format_f64;

    #[test]
    #[allow(clippy::excessive_precision)] // the long spelling is the point
    fn shortest_round_trip() {
        // §4.1 rule 4, and the four values the conformance corpus pins.
        assert_eq!(format_f64(1.05), "1.05");
        assert_eq!(format_f64(0.10000000000000001), "0.1");
        assert_eq!(format_f64(0.0), "0.0");
        assert_eq!(format_f64(1e-8), "1e-8");
        assert_eq!(format_f64(1000000.0), "1000000.0");
        assert_eq!(format_f64(45.0), "45.0");
        assert_eq!(format_f64(-0.0), "-0.0");
    }

    #[test]
    fn every_printed_float_parses_back_bit_for_bit() {
        let mut x = 1.0f64;
        for _ in 0..2000 {
            x = x * 1.2345_f64 + 0.5;
            let text = format_f64(x);
            assert_eq!(
                text.parse::<f64>().unwrap().to_bits(),
                x.to_bits(),
                "{text}"
            );
            let small = 1.0 / x;
            let text = format_f64(small);
            assert_eq!(
                text.parse::<f64>().unwrap().to_bits(),
                small.to_bits(),
                "{text}"
            );
        }
    }

    #[test]
    fn a_float_never_prints_as_an_integer() {
        for v in [1.0, 45.0, -3.0, 1e15, 1e16] {
            let text = format_f64(v);
            assert!(
                text.contains('.') || text.contains('e'),
                "{text} would re-parse as an integer"
            );
        }
    }
}
