//! Scalar semantics for every op the kernel executes.
//!
//! There is exactly **one** definition of what `ln` or `round` means, and both
//! the lane loop and the scalar trap replay (`03-engine.md` §5.5) call it. That
//! is not tidiness: if the replay computed anything differently from the hot
//! loop, the operand values in an `E0902` report would be a re-derivation rather
//! than the real ones, and the report would be a guess.
//!
//! Every function returns `(value, Option<TrapKind>)`. A trap does not stop
//! anything here — the caller sets a lane bit and carries the poison value on
//! (`01-ir.md` §9.3): the hot loop is branch-free by construction.

use predictable_tape::{CmpOp, Fn1, Fn2, FnN};

use crate::traps::TrapKind;

/// A value and, if the operation trapped, why.
pub type Outcome = (f64, Option<TrapKind>);

#[inline]
fn ok(x: f64) -> Outcome {
    (x, None)
}

/// `a / b`. Zero divisor is `div_by_zero`; a finite pair producing an infinity
/// is `overflow_to_inf`. NaN and ±Inf never silently enter results (§9).
#[inline]
pub fn div(a: f64, b: f64) -> Outcome {
    if b == 0.0 {
        return (f64::NAN, Some(TrapKind::DivByZero));
    }
    let r = a / b;
    if !r.is_finite() && a.is_finite() && b.is_finite() {
        return (r, Some(TrapKind::OverflowToInf));
    }
    ok(r)
}

/// `a ^ b`. A NaN out of non-NaN operands is `pow_nan` — the classic
/// `(-1) ^ 0.5`.
///
/// `libm::pow`, not `f64::powf`: the platform libms disagree in the last ULP
/// (glibc vs. macOS), and `03-engine.md` §7 rule 4 requires the vendored one.
#[inline]
pub fn pow(a: f64, b: f64) -> Outcome {
    let r = libm::pow(a, b);
    if r.is_nan() && !a.is_nan() && !b.is_nan() {
        return (r, Some(TrapKind::PowNan));
    }
    if !r.is_finite() && a.is_finite() && b.is_finite() {
        return (r, Some(TrapKind::OverflowToInf));
    }
    ok(r)
}

/// Comparison. Booleans live in `f64` lanes as `0.0` / `1.0`, so a comparison
/// is just another arithmetic op and `Select` needs no predicate stack.
#[inline]
pub fn cmp(op: CmpOp, a: f64, b: f64) -> f64 {
    let t = match op {
        CmpOp::Eq => a == b,
        CmpOp::Ne => a != b,
        CmpOp::Lt => a < b,
        CmpOp::Le => a <= b,
        CmpOp::Gt => a > b,
        CmpOp::Ge => a >= b,
    };
    f64::from(t)
}

/// True is anything non-zero, so `and`/`or` can be written arithmetically.
#[inline]
pub fn truthy(x: f64) -> bool {
    x != 0.0
}

/// The unary builtins of `01-ir.md` §2.8.
pub fn apply1(f: Fn1, x: f64) -> Outcome {
    match f {
        Fn1::Abs => ok(x.abs()),
        Fn1::Floor => ok(x.floor()),
        Fn1::Ceil => ok(x.ceil()),
        Fn1::Sign => ok(if x > 0.0 {
            1.0
        } else if x < 0.0 {
            -1.0
        } else {
            0.0
        }),
        Fn1::Exp => {
            // Vendored libm, not the platform's (`03-engine.md` §7 rule 4).
            let r = libm::exp(x);
            if r.is_infinite() {
                (r, Some(TrapKind::OverflowToInf))
            } else {
                ok(r)
            }
        }
        Fn1::Ln => {
            if x <= 0.0 {
                (f64::NAN, Some(TrapKind::LogNonPositive))
            } else {
                // Vendored libm, not the platform's (`03-engine.md` §7 rule 4).
                ok(libm::log(x))
            }
        }
        Fn1::Sqrt => {
            if x < 0.0 {
                (f64::NAN, Some(TrapKind::NotFinite))
            } else {
                ok(x.sqrt())
            }
        }
        // Rate conversions are compounded, never divided: the divided form is
        // `nominal_to_periodic`, which the model must name explicitly (§2.8).
        Fn1::ToMonthly => pow(1.0 + x, 1.0 / 12.0).0.pipe(|v| ok(v - 1.0)),
        Fn1::ToAnnual => pow(1.0 + x, 12.0).0.pipe(|v| ok(v - 1.0)),
        Fn1::VFromI => div(1.0, 1.0 + x),
        Fn1::IFromV => match div(1.0, x) {
            (v, None) => ok(v - 1.0),
            (v, t) => (v, t),
        },
        Fn1::Year => ok(crate::dates::civil_from_days(x as i64).0 as f64),
        Fn1::Month => ok(crate::dates::civil_from_days(x as i64).1 as f64),
        Fn1::Day => ok(crate::dates::civil_from_days(x as i64).2 as f64),
        // There are no nulls (§2.11); the presence bit a lookup produces is the
        // only thing `is_null` observes, and a non-lookup value is never null.
        Fn1::IsNull => ok(f64::from(x.is_nan())),
    }
}

/// The binary builtins.
pub fn apply2(f: Fn2, a: f64, b: f64) -> Outcome {
    match f {
        Fn2::Min => ok(if a < b { a } else { b }),
        Fn2::Max => ok(if a > b { a } else { b }),
        Fn2::Round => ok(round_half_away(a, b as i32)),
        // `r / n`, named so the choice is visible in the diff (§2.8).
        Fn2::NominalToPeriodic => div(a, b),
        Fn2::AnnuityFactor => {
            if a == 0.0 {
                ok(b)
            } else {
                match pow(1.0 + a, -b) {
                    (v, None) => div(1.0 - v, a),
                    (v, t) => (v, t),
                }
            }
        }
        Fn2::Compound => pow(1.0 + a, b),
        Fn2::Coalesce => ok(if a.is_nan() { b } else { a }),
        Fn2::AddMonths => ok(crate::dates::add_months(a as i64, b as i64) as f64),
        Fn2::MonthsBetween => ok(crate::dates::months_between(a as i64, b as i64) as f64),
    }
}

/// The wider builtins.
pub fn applyn(f: FnN, args: &[f64]) -> Outcome {
    match f {
        FnN::Clamp => {
            let (x, lo, hi) = (args[0], args[1], args[2]);
            ok(if x < lo {
                lo
            } else if x > hi {
                hi
            } else {
                x
            })
        }
        // `year_frac(a, b, convention)`; the convention arrives as a dictionary
        // code the caller has already resolved to a divisor.
        FnN::YearFrac => {
            let days = args[1] - args[0];
            let basis = if args.len() > 2 && args[2] != 0.0 {
                args[2]
            } else {
                365.0
            };
            div(days, basis)
        }
    }
}

/// `round(x, dp)`: **round-half-away-from-zero on the shortest decimal
/// representation of `x`** (`01-ir.md` §2.8), not IEEE round-half-even.
///
/// The distinction is the `2.675` trap: `2.675` is really
/// `2.67499999999999982...`, so scaling and rounding gives `2.67`, while the
/// actuarial convention — and Prophet — give `2.68`. Rounding the *shortest*
/// decimal text, which is what the model author wrote, gives `2.68`.
pub fn round_half_away(x: f64, dp: i32) -> f64 {
    if !x.is_finite() {
        return x;
    }
    if dp < 0 {
        let scale = 10f64.powi(-dp);
        return round_half_away(x / scale, 0) * scale;
    }
    let text = format!("{x}"); // Rust's shortest round-tripping decimal
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => (-1.0, rest.to_string()),
        None => (1.0, text),
    };
    if digits.contains(['e', 'E']) {
        // Beyond the decimal range where the shortest form is written out, the
        // value has no fractional digits to round.
        return x;
    }
    let (int_part, frac_part) = match digits.split_once('.') {
        Some((i, f)) => (i.to_string(), f.to_string()),
        None => (digits, String::new()),
    };
    let dp = dp as usize;
    if frac_part.len() <= dp {
        return x;
    }
    let mut kept: Vec<u8> = format!("{int_part}{}", &frac_part[..dp]).into_bytes();
    let next = frac_part.as_bytes()[dp];
    if next >= b'5' {
        let mut i = kept.len();
        loop {
            if i == 0 {
                kept.insert(0, b'1');
                break;
            }
            i -= 1;
            if kept[i] == b'9' {
                kept[i] = b'0';
            } else {
                kept[i] += 1;
                break;
            }
        }
    }
    let s = String::from_utf8(kept).expect("ascii digits");
    let value: f64 = s.parse().unwrap_or(0.0);
    sign * value / 10f64.powi(dp as i32)
}

/// A tiny pipe so the rate conversions read as the formulae they are.
trait Pipe: Sized {
    fn pipe<R>(self, f: impl FnOnce(Self) -> R) -> R {
        f(self)
    }
}
impl Pipe for f64 {}
