//! A generator of random *valid* programs, for the property test §7 clause 1 asks for.
//!
//! `prop_assert_eq!(run(m, C=1), run(m, C=1024))` needs an `m` supply. Hand-written models test
//! the constructs someone thought of; generated ones test the shapes nobody did — a lag chained
//! through an `if` through a stage-2 reduction, at a depth no human writes by hand.
//!
//! The generator is deliberately its own tiny LCG rather than a dependency: a property test whose
//! failures cannot be reproduced from a printed seed is a flake, and the seed must mean the same
//! thing in a year's time. `program(7)` is the same program on every machine, forever.
//!
//! It generates only programs the checker accepts — every value is `f64` with `unit = "none"`,
//! every self-reference is lag-1 with an `init`, every reference points backwards. That is a
//! real restriction: it explores expression shape, not the checker's error surface, which is
//! `predictable-check`'s corpus to own.

/// A reproducible xorshift-flavoured LCG. Same seed, same program, everywhere.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    /// Seed the generator. Any `u64` is a valid seed.
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1))
    }

    /// The next value in `0..n`.
    pub fn below(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % n.max(1)
    }

    /// A small, exactly-representable-ish constant. Ugly on purpose: `0.1` and friends are where
    /// reassociation shows up.
    pub fn constant(&mut self) -> f64 {
        const POOL: [f64; 8] = [0.1, 0.5, 1.0, 2.0, 0.03125, 1e8, 1e-8, 3.7];
        POOL[self.below(POOL.len())]
    }
}

const FIELDS: [&str; 4] = ["mp_a", "mp_b", "mp_c", "mp_d"];

/// A generated program: `.pir` source plus the component names it emits.
#[derive(Debug, Clone)]
pub struct Program {
    /// The seed it came from — print this in any failure message.
    pub seed: u64,
    /// `.pir` module source.
    pub source: String,
}

/// Generate one program from a seed.
pub fn program(seed: u64) -> Program {
    let mut rng = Rng::new(seed);
    let periods = 3 + rng.below(6);
    let series_count = 2 + rng.below(4);

    let mut s = String::new();
    s.push_str("format = \"pir/1\"\nmodule = \"gen\"\n\n");
    s.push_str(&format!(
        "[timeline]\nbasis = \"annual\"\nperiods = {periods}\norigin = \"policy\"\nvaluation_date = 2026-06-30\n\n"
    ));
    s.push_str("[[modelpoint_field]]\nname = \"policy_number\"\ndtype = \"str\"\nrequired = true\nkey = true\n\n");
    for f in FIELDS {
        s.push_str(&format!(
            "[[modelpoint_field]]\nname = \"{f}\"\ndtype = \"f64\"\nunit = \"none\"\nrequired = true\n\n"
        ));
    }

    let mut series: Vec<String> = Vec::new();
    for i in 0..series_count {
        let name = format!("s{i}");
        let expr = expr(&mut rng, 3, &series, Some(&name));
        s.push_str(&format!(
            "[[component]]\nname = \"{name}\"\nkind = \"Derived\"\ndtype = \"f64\"\nshape = \"Series\"\nunit = \"none\"\ntiming = \"end\"\ninit = \"{init}\"\nexpr = \"{expr}\"\n\n",
            init = rng.constant()
        ));
        series.push(name);
    }

    // At least one Series and one PerMP output, so the golden covers both strides and stage 2
    // actually runs.
    let last = series.last().unwrap().clone();
    let out = expr(&mut rng, 2, &series, None);
    s.push_str(&format!(
        "[[component]]\nname = \"out\"\nkind = \"Output\"\ndtype = \"f64\"\nshape = \"Series\"\nunit = \"none\"\ntiming = \"end\"\nexpr = \"{out}\"\n\n"
    ));
    let reduction = match rng.below(5) {
        0 => format!("sum({last})"),
        1 => format!("sum_kahan({last})"),
        2 => format!("npv({last}, {})", series[0]),
        3 => format!("max_over({last})"),
        _ => format!("last({last})"),
    };
    s.push_str(&format!(
        "[[component]]\nname = \"total\"\nkind = \"Output\"\ndtype = \"f64\"\nshape = \"PerMP\"\nunit = \"none\"\nexpr = \"{reduction}\"\n"
    ));

    Program { seed, source: s }
}

/// One `f64` expression of at most `depth` nesting.
///
/// `self_name` is the component being defined: it may only be read at lag 1, which is what keeps
/// the generated program acyclic by construction rather than by rejection sampling.
fn expr(rng: &mut Rng, depth: usize, series: &[String], self_name: Option<&str>) -> String {
    if depth == 0 {
        return leaf(rng, series, self_name);
    }
    let a = expr(rng, depth - 1, series, self_name);
    let b = expr(rng, depth - 1, series, self_name);
    match rng.below(10) {
        0 => format!("({a} + {b})"),
        1 => format!("({a} - {b})"),
        2 => format!("({a} * {b})"),
        3 => format!("min({a}, {b})"),
        4 => format!("max({a}, {b})"),
        5 => format!("abs({a})"),
        // Guarded so the generator explores arithmetic, not the trap machinery: a generated
        // division by zero would test `on_trap`, which is `predictable-engine`'s own suite.
        6 => format!("({a} / (1.0 + abs({b})))"),
        7 => format!("sqrt(abs({a}))"),
        8 => format!("(if {a} > {b} then {a} else {b})"),
        _ => leaf(rng, series, self_name),
    }
}

fn leaf(rng: &mut Rng, series: &[String], self_name: Option<&str>) -> String {
    // The lag arm is only offered when there is a `self` to lag, and it is always lag 1.
    let arms = if self_name.is_some() { 4 } else { 3 };
    match rng.below(arms) {
        0 => format!("{:?}", rng.constant()),
        1 => FIELDS[rng.below(FIELDS.len())].to_string(),
        2 => {
            if series.is_empty() {
                FIELDS[rng.below(FIELDS.len())].to_string()
            } else {
                series[rng.below(series.len())].clone()
            }
        }
        _ => format!("{}[t-1]", self_name.unwrap()),
    }
}
