//! `[[solve]]`: Brent's method wrapped around the whole projection (`03-engine.md` §5.4,
//! `01-ir.md` §8.2, §8.4.4 — ruling Q8).
//!
//! A solve is a root-find *outside* the engine. The projection stays a pure function of its
//! inputs; the solver perturbs one input, re-runs the chunk pipeline, and reads the residual
//! `target - to`. Each iteration is itself deterministic, so the solved value is reproducible.
//!
//! Two scopes, one algorithm:
//!
//! - `scope = "per_mp"` — every modelpoint has its own bracket, its own iterate and its own
//!   convergence verdict. They are advanced **in lockstep**: one pipeline pass evaluates the
//!   residual for every unconverged modelpoint at its own candidate `x`. That costs `max_iter`
//!   passes at worst rather than `max_iter × n_modelpoints`, and it changes no answer, because
//!   modelpoints are independent (`01-ir.md` §8.3).
//! - `scope = "portfolio"` — one scalar Brent over an `[[aggregation]]` value.
//!
//! [`BrentState`] is the per-root state machine that makes the lockstep possible: `step` takes
//! the residual at the last proposal and returns the next proposal, so the caller owns the
//! evaluation and can batch it.

/// Why a root-find stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Still bracketing; `x` is the next point to evaluate.
    Running,
    /// `|residual| <= tolerance`.
    Converged,
    /// `max_iter` exhausted (`01-ir.md` §8.4.4 counts these in `not_converged`).
    MaxIter,
    /// `f(a)` and `f(b)` have the same sign: the declared `bracket` does not contain a root.
    NoBracket,
}

impl Status {
    /// True once the state machine will propose nothing further.
    pub fn is_done(self) -> bool {
        !matches!(self, Status::Running)
    }
}

/// One root-find, as a state machine driven by residuals.
///
/// This is the classic van Wijngaarden–Dekker–Brent method: inverse quadratic interpolation
/// where it is well-behaved, secant where it is not, bisection whenever the interpolant would
/// step outside the bracket or converge too slowly. No randomness, no adaptive tolerances, no
/// wall-clock cut-off — every branch is a comparison of `f64`s that are themselves reproducible.
#[derive(Debug, Clone)]
pub struct BrentState {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    fa: f64,
    fb: f64,
    fc: f64,
    tolerance: f64,
    max_iter: u32,
    iterations: u32,
    phase: Phase,
    status: Status,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    NeedFa,
    NeedFb,
    Iterating,
}

impl BrentState {
    /// A solve over `[lo, hi]`, converged when `|residual| <= tolerance`.
    pub fn new(bracket: [f64; 2], tolerance: f64, max_iter: u32) -> BrentState {
        let [a, b] = bracket;
        BrentState {
            a,
            b,
            c: b,
            d: b - a,
            e: b - a,
            fa: f64::NAN,
            fb: f64::NAN,
            fc: f64::NAN,
            tolerance,
            max_iter,
            iterations: 0,
            phase: Phase::NeedFa,
            status: Status::Running,
        }
    }

    /// The point whose residual the caller must supply to [`BrentState::step`].
    pub fn proposal(&self) -> f64 {
        match self.phase {
            Phase::NeedFa => self.a,
            _ => self.b,
        }
    }

    /// The best iterate so far — the solved value once [`BrentState::status`] is `Converged`.
    pub fn solution(&self) -> f64 {
        self.b
    }

    /// The residual at [`BrentState::solution`].
    pub fn residual(&self) -> f64 {
        self.fb
    }

    /// Projection passes this root has consumed.
    pub fn iterations(&self) -> u32 {
        self.iterations
    }

    /// Where the root-find stands.
    pub fn status(&self) -> Status {
        self.status
    }

    /// Feed the residual at [`BrentState::proposal`]; the proposal moves on.
    pub fn step(&mut self, residual: f64) -> Status {
        if self.status.is_done() {
            return self.status;
        }
        self.iterations += 1;
        match self.phase {
            Phase::NeedFa => {
                self.fa = residual;
                if residual.abs() <= self.tolerance {
                    // The lower bracket is itself the root; report it as the solution.
                    self.b = self.a;
                    self.fb = residual;
                    self.status = Status::Converged;
                    return self.status;
                }
                self.phase = Phase::NeedFb;
            }
            Phase::NeedFb => {
                self.fb = residual;
                if residual.abs() <= self.tolerance {
                    self.status = Status::Converged;
                    return self.status;
                }
                if (self.fa > 0.0) == (self.fb > 0.0) {
                    self.status = Status::NoBracket;
                    return self.status;
                }
                self.c = self.a;
                self.fc = self.fa;
                self.d = self.b - self.a;
                self.e = self.d;
                self.phase = Phase::Iterating;
                self.advance();
            }
            Phase::Iterating => {
                self.fb = residual;
                if residual.abs() <= self.tolerance {
                    self.status = Status::Converged;
                    return self.status;
                }
                self.advance();
            }
        }
        if self.iterations >= self.max_iter && !self.status.is_done() {
            self.status = Status::MaxIter;
        }
        self.status
    }

    /// One Brent iteration: choose the next `b` given `(a, b, c)` and their residuals.
    fn advance(&mut self) {
        if (self.fb > 0.0) == (self.fc > 0.0) {
            self.c = self.a;
            self.fc = self.fa;
            self.d = self.b - self.a;
            self.e = self.d;
        }
        if self.fc.abs() < self.fb.abs() {
            self.a = self.b;
            self.b = self.c;
            self.c = self.a;
            self.fa = self.fb;
            self.fb = self.fc;
            self.fc = self.fa;
        }
        let tol1 = 2.0 * f64::EPSILON * self.b.abs() + 0.5 * self.tolerance;
        let xm = 0.5 * (self.c - self.b);
        if xm.abs() <= tol1 {
            self.status = Status::Converged;
            return;
        }
        if self.e.abs() >= tol1 && self.fa.abs() > self.fb.abs() {
            let s = self.fb / self.fa;
            let (mut p, mut q);
            if self.a == self.c {
                // Secant.
                p = 2.0 * xm * s;
                q = 1.0 - s;
            } else {
                // Inverse quadratic interpolation.
                let qq = self.fa / self.fc;
                let r = self.fb / self.fc;
                p = s * (2.0 * xm * qq * (qq - r) - (self.b - self.a) * (r - 1.0));
                q = (qq - 1.0) * (r - 1.0) * (s - 1.0);
            }
            if p > 0.0 {
                q = -q;
            }
            p = p.abs();
            let acceptable = 2.0 * p < (3.0 * xm * q - (tol1 * q).abs()).min((self.e * q).abs());
            if acceptable {
                self.e = self.d;
                self.d = p / q;
            } else {
                self.d = xm;
                self.e = self.d;
            }
        } else {
            self.d = xm;
            self.e = self.d;
        }
        self.a = self.b;
        self.fa = self.fb;
        self.b += if self.d.abs() > tol1 {
            self.d
        } else if xm >= 0.0 {
            tol1
        } else {
            -tol1
        };
    }
}

/// Solve a scalar residual function — the `scope = "portfolio"` path, and the reference the
/// lockstep per-modelpoint driver is tested against.
pub fn brent<F>(bracket: [f64; 2], tolerance: f64, max_iter: u32, mut f: F) -> BrentState
where
    F: FnMut(f64) -> f64,
{
    let mut state = BrentState::new(bracket, tolerance, max_iter);
    while !state.status().is_done() {
        let x = state.proposal();
        state.step(f(x));
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_polynomial_root() {
        let s = brent([0.0, 10.0], 1e-12, 100, |x| x * x - 4.0);
        assert_eq!(s.status(), Status::Converged);
        assert!((s.solution() - 2.0).abs() < 1e-8, "{}", s.solution());
    }

    #[test]
    fn finds_a_transcendental_root_in_few_iterations() {
        let s = brent([0.0, 3.0], 1e-12, 100, |x| x.exp() - 5.0);
        assert_eq!(s.status(), Status::Converged);
        assert!((s.solution() - 5.0_f64.ln()).abs() < 1e-8);
        // Brent, not bisection: 1e-12 on this function is nowhere near 40 halvings.
        assert!(s.iterations() < 20, "{} iterations", s.iterations());
    }

    #[test]
    fn an_endpoint_that_is_already_the_root_converges_immediately() {
        let s = brent([2.0, 10.0], 1e-12, 100, |x| x - 2.0);
        assert_eq!(s.status(), Status::Converged);
        assert_eq!(s.solution(), 2.0);
        assert_eq!(s.iterations(), 1);
    }

    #[test]
    fn a_bracket_without_a_sign_change_is_reported_not_guessed() {
        let s = brent([3.0, 10.0], 1e-12, 100, |x| x * x + 1.0);
        assert_eq!(s.status(), Status::NoBracket);
    }

    #[test]
    fn max_iter_is_honoured() {
        let s = brent([0.0, 1.0], 0.0, 5, |x| x - std::f64::consts::FRAC_1_PI);
        assert!(matches!(s.status(), Status::MaxIter | Status::Converged));
        assert!(s.iterations() <= 5);
    }

    #[test]
    fn the_state_machine_and_the_driver_agree() {
        let f = |x: f64| x.powi(3) - 2.0 * x - 5.0;
        let driven = brent([1.0, 4.0], 1e-10, 100, f);
        let mut manual = BrentState::new([1.0, 4.0], 1e-10, 100);
        while !manual.status().is_done() {
            let x = manual.proposal();
            manual.step(f(x));
        }
        assert_eq!(driven.solution().to_bits(), manual.solution().to_bits());
    }
}
