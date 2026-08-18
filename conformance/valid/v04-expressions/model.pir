format = "pir/1"
module = "expressions"

[timeline]
basis = "annual"
periods = 20
origin = "policy"
valuation_date = 2026-06-30
year_convention = "act/365"

[[enum]]
name = "Gender"
values = ["M", "F"]

[[modelpoint_field]]
name = "policy_number"
dtype = "str"
required = true
key = true

[[modelpoint_field]]
name = "amount"
dtype = "f64"
unit = "money"
required = true

[[modelpoint_field]]
name = "band"
dtype = "i64"
required = true

[[modelpoint_field]]
name = "term_years"
dtype = "i64"
unit = "years"
required = true

[[modelpoint_field]]
name = "gender"
dtype = "enum(Gender)"
required = true

[[modelpoint_field]]
name = "smoker"
dtype = "bool"
required = true

[[modelpoint_field]]
name = "issue_date"
dtype = "date"
required = true

[[modelpoint_field]]
name = "opt_loading"
dtype = "f64"
unit = "factor"
default = 1.0

[[assumption]]
name = "annual_rate"
dtype = "f64"
unit = "rate(annual)"
shape = "Scalar"

[[table]]
name = "rating"
keys = [{ name = "band", dtype = "i64", policy = "step" }]
values = [{ name = "load", dtype = "f64", unit = "factor" }]
on_missing = "default(1.0)"
source = "inline"
digest = "sha256:7d3f4c2b1e908a56d4c3b2a1f0e9d8c7b6a5948372615043f2e1d0c9b8a77665"
rows = [
  [1, 0.9],
  [2, 1.0],
  [3, 1.25],
]

# ---- Lit, Ref, Binary, Unary, If ---------------------------------------
[[component]]
name = "lit_scalar"
kind = "Output"
dtype = "f64"
shape = "Scalar"
unit = "none"
expr = "3.0"
doc = "Lit."

[[component]]
name = "neg_amount"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "-amount"
doc = "Unary over a Ref to a modelpoint field."

[[component]]
name = "flow"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "amount * (if t == 0 then 1.0 else 0.95)"
doc = "If as a value conditional; both arms are evaluated (IR §2.6)."

# ---- Lag and At --------------------------------------------------------
[[component]]
name = "prior_flow"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "flow[t-1]"
doc = "Lag with k = 1. At t = 0 this is the f64 zero, with a pre_origin_default note (IR §2.7)."

[[component]]
name = "flow_at_origin"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "flow[0]"
doc = "At with an absolute index."

# ---- arithmetic builtins ----------------------------------------------
[[component]]
name = "bounded"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "min(max(amount, 0.0), 1000000.0)"

[[component]]
name = "magnitude"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "abs(neg_amount) * sign(amount)"

[[component]]
name = "rounded"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "round(amount, 2)"
doc = "Round-half-away-from-zero on the shortest decimal representation (IR §2.8)."

[[component]]
name = "thousands"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "factor"
expr = "floor(amount / 1000.0) + ceil(amount / 1000.0)"
doc = "money / money = factor."

[[component]]
name = "clamped_loading"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "factor"
expr = "clamp(opt_loading, 0.5, 2.0)"

# ---- exp / log ---------------------------------------------------------
[[component]]
name = "growth"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "factor"
expr = "exp(ln(sqrt(pow(1.05, 2.0)))) * clamped_loading"

# ---- rate builtins -----------------------------------------------------
[[component]]
name = "monthly_rate"
kind = "Output"
dtype = "f64"
shape = "Scalar"
unit = "rate(monthly)"
expr = "to_monthly(annual_rate)"

[[component]]
name = "back_to_annual"
kind = "Output"
dtype = "f64"
shape = "Scalar"
unit = "rate(annual)"
expr = "to_annual(monthly_rate)"

[[component]]
name = "nominal_periodic"
kind = "Output"
dtype = "f64"
shape = "Scalar"
unit = "rate(monthly)"
expr = "nominal_to_periodic(annual_rate, 12)"

[[component]]
name = "v"
kind = "Output"
dtype = "f64"
shape = "Scalar"
unit = "factor"
expr = "v_from_i(annual_rate)"

[[component]]
name = "i_again"
kind = "Output"
dtype = "f64"
shape = "Scalar"
unit = "rate(annual)"
expr = "i_from_v(v)"

[[component]]
name = "annuity"
kind = "Output"
dtype = "f64"
shape = "Scalar"
unit = "factor"
expr = "annuity_factor(annual_rate, 20)"

[[component]]
name = "accumulation"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "factor"
expr = "compound(annual_rate, term_years)"

# ---- timing builtins ---------------------------------------------------
[[component]]
name = "shifted"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "shift(flow, 1)"

[[component]]
name = "retimed"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "retime(flow, end)"

[[component]]
name = "cumulative"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "cum(flow)"

[[component]]
name = "increment"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "diff(flow)"

# ---- logic -------------------------------------------------------------
[[component]]
name = "in_term"
kind = "Output"
dtype = "bool"
shape = "Series"
unit = "none"
timing = "start"
expr = "t < term_years"

[[component]]
name = "logic_mix"
kind = "Output"
dtype = "bool"
shape = "Series"
unit = "none"
timing = "start"
expr = "not (flow > 0.0) or flow >= 0.0 and flow <= amount"

[[component]]
name = "equality"
kind = "Output"
dtype = "bool"
shape = "PerMP"
unit = "none"
expr = "gender == \"M\" and smoker != true"
doc = "Enums compare by equality only; they are never ordered (IR §2.9)."

# ---- presence bits -----------------------------------------------------
[[component]]
name = "loading_absent"
kind = "Output"
dtype = "bool"
shape = "PerMP"
unit = "none"
expr = "is_null(opt_loading)"
doc = "Legal: `opt_loading` is an optional modelpoint field, so it has a presence bit (IR §2.11)."

[[component]]
name = "loading_or_one"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "factor"
expr = "coalesce(opt_loading, 1.0)"

[[component]]
name = "rating_absent"
kind = "Output"
dtype = "bool"
shape = "PerMP"
unit = "none"
expr = "is_null(rating@(band))"
doc = "Legal: `rating` has on_missing = default(...), so the lookup carries a hit flag."

# ---- lookup ------------------------------------------------------------
[[component]]
name = "band_load"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "factor"
expr = "rating@(band)"

# ---- dates -------------------------------------------------------------
[[component]]
name = "issue_year"
kind = "Output"
dtype = "i64"
shape = "PerMP"
unit = "none"
expr = "year(issue_date)"

[[component]]
name = "issue_month"
kind = "Output"
dtype = "i64"
shape = "PerMP"
unit = "none"
expr = "month(issue_date)"

[[component]]
name = "issue_day"
kind = "Output"
dtype = "i64"
shape = "PerMP"
unit = "none"
expr = "day(issue_date)"

[[component]]
name = "first_anniversary"
kind = "Output"
dtype = "date"
shape = "PerMP"
unit = "none"
expr = "add_months(issue_date, 12)"

[[component]]
name = "months_to_anniversary"
kind = "Output"
dtype = "i64"
shape = "PerMP"
unit = "months"
expr = "months_between(issue_date, first_anniversary)"

[[component]]
name = "years_to_anniversary"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "years"
expr = "year_frac(issue_date, first_anniversary, \"act/365\")"

# ---- discounting and aggregates ---------------------------------------
[[component]]
name = "disc"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "factor"
timing = "point"
init = "1.0"
expr = "disc[t-1] * v"

[[component]]
name = "total"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum(flow)"

[[component]]
name = "total_kahan"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum_kahan(flow)"

[[component]]
name = "total_in_term"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum(flow, in_term)"
doc = "Predicated Agg: the optional second operand is the predicate (IR §2.6)."

[[component]]
name = "pv"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "npv(flow, disc)"
doc = "`flow` is start-timed, so the exponent is v^t (IR §2.5)."

[[component]]
name = "first_flow"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "first(flow)"

[[component]]
name = "last_flow"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "last(flow)"

[[component]]
name = "flow_at_five"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "at(flow, 5)"
doc = "`at(x, k)` and `x[k]` are the same operation and lower to the same At node (IR §2.8)."

[[component]]
name = "peak_flow"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "max_over(flow)"

[[component]]
name = "trough_flow"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "min_over(flow)"

[[component]]
name = "years_in_term"
kind = "Output"
dtype = "i64"
shape = "PerMP"
unit = "count"
expr = "count_while(in_term)"
doc = "Stops at the first t where the predicate is false and returns that count (IR §2.8)."

[[component]]
name = "safe_ratio"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "factor"
expr = "if total == 0.0 then 0.0 else last_flow / total"
doc = "Trap suppression in the untaken arm: the division is masked syntactically (IR §2.6)."
