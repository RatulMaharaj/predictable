//! Rendering a whole projection to text, for the determinism gate.
//!
//! `03-engine.md` §7 clause 7 and T16: a golden is the byte-exact rendering of
//! a run, with every value written as its IEEE-754 bit pattern beside its
//! decimal form, because a decimal comparison passes on a platform that got the
//! last ULP wrong. This module is that renderer, restricted to what the wasm
//! build can reach (no `predictable-io`, no runner) so the *same function*
//! compiles for `aarch64-apple-darwin` and `wasm32-wasip1` and the gate can
//! compare the two byte for byte.

use crate::session::{Inputs, Session, SessionError};

/// The rendering. Stable across releases only in the sense a golden is: a
/// change here is a change to every gate expectation and is reviewed as one.
pub fn render_case(name: &str, inputs: &Inputs) -> Result<String, SessionError> {
    let session = Session::new(inputs)?;
    let projection = session.project(&[], &Default::default())?;

    let mut out = String::new();
    out.push_str("# predictable wasm determinism render\n");
    out.push_str(&format!("case          {name}\n"));
    out.push_str(&format!("model_digest  {}\n", session.model_digest()));
    out.push_str(&format!("plan_digest   {}\n", session.plan_digest()));
    out.push_str(&format!("order_digest  {}\n", session.order_digest()));
    out.push_str(&format!("periods       {}\n", session.periods()));
    out.push_str(&format!("modelpoints   {}\n", projection.keys.len()));
    out.push_str(&format!("dropped       {}\n", projection.dropped.join(",")));

    for (lane, key) in projection.keys.iter().enumerate() {
        out.push_str(&format!("\n[mp {key}]\n"));
        for (component, values) in &projection.columns {
            let stride = projection.strides.get(component).copied().unwrap_or(1);
            let lane_values = &values[lane * stride..(lane + 1) * stride];
            for (t, value) in lane_values.iter().enumerate() {
                out.push_str(&format!(
                    "{component} t={t} {} {:?}\n",
                    bits(*value),
                    Canonical(*value)
                ));
            }
        }
    }
    Ok(out)
}

/// A value's IEEE-754 bits as 16 hex digits — the assertion the gate makes.
pub fn bits(value: f64) -> String {
    format!("{:016x}", value.to_bits())
}

/// `{:?}` on `f64` is shortest-round-trip and identical on every target, but
/// NaN and infinities print without a sign in some formatters. Wrapping them
/// makes the text total.
struct Canonical(f64);

impl std::fmt::Debug for Canonical {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.is_nan() {
            f.write_str(if self.0.is_sign_negative() {
                "-nan"
            } else {
                "nan"
            })
        } else if self.0.is_infinite() {
            f.write_str(if self.0.is_sign_negative() {
                "-inf"
            } else {
                "inf"
            })
        } else {
            write!(f, "{:?}", self.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::bits;

    #[test]
    fn bits_are_the_pattern_not_the_decimal() {
        // The canonical example: two values that print the same at 15 places
        // and differ in the last bit.
        let a = 0.1_f64 + 0.2;
        let b = 0.3_f64;
        assert_ne!(bits(a), bits(b));
        assert_eq!(bits(b), "3fd3333333333333");
    }
}
