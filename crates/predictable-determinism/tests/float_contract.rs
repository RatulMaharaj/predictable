//! The float contract: clauses 2, 3 and 4 of `03-engine.md` §7.
//!
//! These are the tests that would catch a compiler flag change nobody meant to make. They assert
//! on bit patterns, never on tolerances — a tolerance here would defeat the purpose.

mod common;

use common::{lane, run, scalar};
use predictable_determinism::{assert_golden, bits};

/// Four modelpoint fields, read one per period, so a test can put chosen bit patterns into a
/// series in a chosen order. `periods = 3` is four points, `t = 0..=3`.
const PICK4: &str = r#"format = "pir/1"
module = "pick"

[timeline]
basis = "annual"
periods = 3
origin = "policy"
valuation_date = 2026-06-30

[[modelpoint_field]]
name = "policy_number"
dtype = "str"
required = true
key = true

[[modelpoint_field]]
name = "v0"
dtype = "f64"
unit = "none"
required = true

[[modelpoint_field]]
name = "v1"
dtype = "f64"
unit = "none"
required = true

[[modelpoint_field]]
name = "v2"
dtype = "f64"
unit = "none"
required = true

[[modelpoint_field]]
name = "v3"
dtype = "f64"
unit = "none"
required = true

[[component]]
name = "flow"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "none"
timing = "end"
expr = "if t == 0 then v0 else (if t == 1 then v1 else (if t == 2 then v2 else v3))"

[[component]]
name = "total"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "none"
expr = "sum(flow)"

[[component]]
name = "total_kahan"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "none"
expr = "sum_kahan(flow)"
"#;

/// The adversarial series `[1e16, 1.0, -1e16, 1.0]`: sequential summation loses the two `1.0`s
/// against `1e16` and returns `1.0`; any reassociation that cancels the big pair first returns
/// `2.0`. A vectorised reduction would show up here as a changed answer, not a changed timing.
const ADVERSARIAL: &str =
    "policy_number,v0,v1,v2,v3\nMP1,1e16,1.0,-1e16,1.0\nMP2,-1e16,1.0,1e16,1.0\n";

/// Left-to-right, exactly as `03-engine.md` §7 clause 2 requires.
fn sequential_sum(values: &[f64]) -> f64 {
    let mut acc = 0.0;
    for v in values {
        acc += *v;
    }
    acc
}

/// Pairwise (tree) summation — a legal-looking optimisation that changes the answer.
fn pairwise_sum(values: &[f64]) -> f64 {
    if values.len() <= 2 {
        return values.iter().sum();
    }
    let (a, b) = values.split_at(values.len() / 2);
    pairwise_sum(a) + pairwise_sum(b)
}

#[test]
fn reduce_is_sequential_on_adversarial_input() {
    let (_, out) = run("reduce", PICK4, Some(ADVERSARIAL));
    for mp in 0..2 {
        let flow = lane(&out, "pick.flow", mp);
        let engine = lane(&out, "pick.total", mp)[0];
        let want = sequential_sum(&flow);
        assert_eq!(
            engine.to_bits(),
            want.to_bits(),
            "lane {mp}: sum({flow:?}) = {} but sequential summation gives {}",
            bits(engine),
            bits(want)
        );
        // The test has teeth: a reassociated fold really does give a different answer here.
        assert_ne!(
            want.to_bits(),
            pairwise_sum(&flow).to_bits(),
            "lane {mp}: the adversarial input stopped being adversarial"
        );
    }
}

/// `sum_kahan` is compensated, so it *should* differ from the plain sum here — and it must be
/// the same compensated answer every time.
#[test]
fn kahan_is_a_different_but_equally_pinned_answer() {
    let (_, out) = run("kahan", PICK4, Some(ADVERSARIAL));
    let plain = lane(&out, "pick.total", 0)[0];
    let kahan = lane(&out, "pick.total_kahan", 0)[0];
    assert_eq!(plain, 1.0, "plain sequential sum");
    assert_eq!(kahan, 2.0, "compensated sum recovers the lost ones");
    assert_ne!(plain.to_bits(), kahan.to_bits());
}

/// Values where `a*b + c` and `fma(a, b, c)` genuinely differ, so the assertion below cannot
/// pass vacuously.
fn fma_witnesses() -> Vec<(f64, f64, f64)> {
    let mut out = Vec::new();
    let pool = [
        (1.0 + f64::EPSILON, 1.0 + f64::EPSILON, -1.0),
        (1.0 / 3.0, 3.0, -1.0),
        (0.1, 0.1, -0.01),
        (1e16 + 2.0, 1.0 + f64::EPSILON, -1e16),
        (
            std::f64::consts::PI,
            std::f64::consts::PI,
            -9.869604401089358,
        ),
    ];
    for (a, b, c) in pool {
        if (a * b + c).to_bits() != a.mul_add(b, c).to_bits() {
            out.push((a, b, c));
        }
    }
    out
}

/// `a * b + c` is a multiply and then an add — never a fused multiply-add (§7 clause 3).
#[test]
fn no_fma_contraction() {
    let witnesses = fma_witnesses();
    assert!(
        witnesses.len() >= 3,
        "only {} of the chosen triples distinguish fma from mul-then-add; the test is not \
         testing what it claims",
        witnesses.len()
    );

    let mut csv = String::from("policy_number,v0,v1,v2,v3\n");
    for (i, (a, b, c)) in witnesses.iter().enumerate() {
        csv.push_str(&format!("MP{i},{a:?},{b:?},{c:?},0.0\n"));
    }
    let source = PICK4.replace(
        r#"expr = "if t == 0 then v0 else (if t == 1 then v1 else (if t == 2 then v2 else v3))""#,
        r#"expr = "v0 * v1 + v2""#,
    );
    let (_, out) = run("fma", &source, Some(&csv));

    for (i, (a, b, c)) in witnesses.iter().enumerate() {
        let got = lane(&out, "pick.flow", i)[0];
        assert_eq!(
            got.to_bits(),
            (a * b + c).to_bits(),
            "lane {i}: a*b + c was contracted"
        );
        assert_ne!(
            got.to_bits(),
            a.mul_add(*b, *c).to_bits(),
            "lane {i}: the engine produced the fused result"
        );
    }
}

/// Transcendentals, pinned to the bit.
///
/// This is the golden that fails first when the libm underneath the engine changes — glibc's
/// `pow` and macOS's differ in the last ULP, and §7 clause 4 says an actuary reconciling to the
/// penny will find it. `predictable-engine` therefore calls the vendored `libm` crate for
/// `exp`/`ln`/`pow` (`sqrt` is the IEEE-exact hardware instruction), so this golden is
/// platform-independent: a failure here means the vendored libm itself changed.
#[test]
fn transcendentals_are_pinned() {
    let source = PICK4.replace(
        r#"expr = "if t == 0 then v0 else (if t == 1 then v1 else (if t == 2 then v2 else v3))""#,
        r#"expr = "exp(v0) + ln(v1) + pow(v2, v3) + sqrt(v1)""#,
    );
    let inputs: [(f64, f64, f64, f64); 6] = [
        (1.0, 2.0, 1.05, 20.0),
        (0.5, 3.0, 1.0000000001, 100.0),
        (-3.25, 1e8, 2.0, 0.5),
        (13.0, 0.001, 10.0, -3.0),
        (0.0, 1.0, 1.0, 1.0),
        (7.125, 1e-8, 0.9, 40.0),
    ];
    let mut csv = String::from("policy_number,v0,v1,v2,v3\n");
    for (i, (a, b, c, d)) in inputs.iter().enumerate() {
        csv.push_str(&format!("MP{i},{a:?},{b:?},{c:?},{d:?}\n"));
    }
    let (_, out) = run("transcendentals", &source, Some(&csv));

    let mut text = String::from("# exp/ln/pow/sqrt, one line per input tuple\n");
    for (i, tuple) in inputs.iter().enumerate() {
        text.push_str(&format!(
            "{tuple:?} => {}\n",
            bits(lane(&out, "pick.flow", i)[0])
        ));
    }
    assert_golden("transcendentals.golden", &text).unwrap();
}

/// The engine reads no clock, no RNG and no environment: the same inputs give the same answer
/// with the environment perturbed underneath it (§7 clause 5).
#[test]
fn results_do_not_depend_on_the_environment() {
    let (_, first) = run("env", PICK4, Some(ADVERSARIAL));
    let before = scalar(&first, "pick.total");
    std::env::set_var("PREDICTABLE_DETERMINISM_PROBE", "1");
    std::env::set_var("TZ", "Pacific/Kiritimati");
    let (_, second) = run("env", PICK4, Some(ADVERSARIAL));
    std::env::remove_var("PREDICTABLE_DETERMINISM_PROBE");
    assert_eq!(before.to_bits(), scalar(&second, "pick.total").to_bits());
}
