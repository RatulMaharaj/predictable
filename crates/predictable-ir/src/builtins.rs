//! The closed builtin function set of §2.8.
//!
//! Membership is fixed for IR 1.0; adding a name is an IR minor-version bump (§10). The list
//! lives here so that the checker, the DSL bridge and `pir.json` consumers all agree on it.

/// Every legal `Call` function name in IR 1.0, sorted for binary search and for stable
/// "did you mean" suggestions.
pub const BUILTINS: &[&str] = &[
    // aggregates
    "at",
    "count_while",
    "first",
    "last",
    "max_over",
    "min_over",
    "npv",
    "sum",
    "sum_kahan",
    // arithmetic
    "abs",
    "ceil",
    "clamp",
    "floor",
    "max",
    "min",
    "round",
    "sign",
    // exp / log
    "exp",
    "ln",
    "pow",
    "sqrt",
    // rates
    "annuity_factor",
    "compound",
    "i_from_v",
    "nominal_to_periodic",
    "to_annual",
    "to_monthly",
    "v_from_i",
    // timing
    "cum",
    "diff",
    "retime",
    "shift",
    // logic
    "and",
    "coalesce",
    "eq",
    "ge",
    "gt",
    "is_null",
    "le",
    "lt",
    "ne",
    "not",
    "or",
    // dates
    "add_months",
    "day",
    "month",
    "months_between",
    "year",
    "year_frac",
];

/// True when `name` is a builtin of IR 1.0.
pub fn is_builtin(name: &str) -> bool {
    BUILTINS.contains(&name)
}

/// The timing operations of §2.5 — the ones that earn `W0105` when applied to an untimed value.
pub const TIMING_OPS: &[&str] = &["retime", "shift", "cum", "diff"];

/// True when `name` is one of the timing operations subject to `W0105` (Q14).
pub fn is_timing_op(name: &str) -> bool {
    TIMING_OPS.contains(&name)
}
