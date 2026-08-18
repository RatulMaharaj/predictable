//! The timeline fields of `01-ir.md` §5, computed rather than stored.
//!
//! `LoadTime` never reads a buffer: every field is a pure function of the
//! `[timeline]` block and `t`, so it costs no memory and cannot drift from the
//! declaration. The clock is built once per run.

use predictable_ir::{Basis, Origin, Timeline};
use predictable_tape::TimeField;

use crate::dates;

/// The timeline, resolved to the numbers the kernel needs.
#[derive(Debug, Clone)]
pub struct Clock {
    pub basis: Basis,
    pub origin: Origin,
    pub periods: u32,
    /// `valuation_date` as days since 1970-01-01.
    pub valuation_days: i64,
    /// Days in a year under `year_convention`; `act/365` → 365.
    pub year_days: f64,
    /// Months in one period: 1, 3 or 12.
    pub months_per_period: i64,
}

impl Clock {
    pub fn new(timeline: &Timeline) -> Clock {
        let months_per_period = 12 / i64::from(timeline.basis.periods_per_year());
        Clock {
            basis: timeline.basis,
            origin: timeline.origin,
            periods: timeline.periods,
            valuation_days: dates::parse_iso(&timeline.valuation_date).unwrap_or(0),
            year_days: match timeline.year_convention.as_str() {
                "act/360" => 360.0,
                "30/360" => 360.0,
                _ => 365.0,
            },
            months_per_period,
        }
    }

    /// Periods per year — the divisor for policy-year arithmetic.
    pub fn periods_per_year(&self) -> u32 {
        self.basis.periods_per_year()
    }

    /// First day of period `t`.
    pub fn period_start(&self, t: u32) -> i64 {
        dates::add_months(self.valuation_days, self.months_per_period * i64::from(t))
    }

    /// One `LoadTime`. The value is broadcast to every lane: timeline fields are
    /// modelpoint-independent by construction, which is also why series that
    /// depend only on them are hoistable (§3.4).
    pub fn field(&self, f: TimeField, t: u32) -> f64 {
        let ppy = self.periods_per_year();
        match f {
            TimeField::T => f64::from(t),
            TimeField::PeriodStartDate => self.period_start(t) as f64,
            TimeField::PeriodEndDate => (self.period_start(t + 1) - 1) as f64,
            TimeField::YearFrac => {
                (self.period_start(t + 1) - self.period_start(t)) as f64 / self.year_days
            }
            TimeField::MonthOfYear => dates::civil_from_days(self.period_start(t)).1 as f64,
            TimeField::PolicyYear => f64::from(t / ppy + 1),
            TimeField::PolicyMonth => f64::from(t) * (12.0 / f64::from(ppy)) + 1.0,
            TimeField::IsAnniversary => f64::from(t > 0 && t % ppy == 0),
        }
    }
}
