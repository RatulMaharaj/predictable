format = "pir/1"
module = "term"

[timeline]
basis = "annual"
periods = 12
origin = "policy"
valuation_date = 2026-06-30

[[modelpoint_field]]
name = "sum_assured"
dtype = "f64"
unit = "money"
required = true

[[modelpoint_field]]
name = "entry_age"
dtype = "i64"
unit = "years"
required = true

[[assumption]]
name = "valuation_rate"
dtype = "f64"
shape = "Scalar"
unit = "rate(annual)"

[[assumption]]
name = "mortality_loading"
dtype = "f64"
shape = "Scalar"
unit = "factor"

[[component]]
name = "annual_rate"
kind = "Derived"
dtype = "f64"
shape = "Scalar"
unit = "rate(annual)"
expr = "valuation_rate * (2.0 + 3.0)"

[[component]]
name = "discount_factor"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "factor"
timing = "start"
expr = "if t == 0 then 1.0 else discount_factor[t-1] / (1.0 + annual_rate)"

[[component]]
name = "q_x"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "prob"
timing = "start"
expr = "0.001 * (1.0 + 0.05 * (entry_age + t)) * mortality_loading"

[[component]]
name = "num_pols_if"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "count"
timing = "start"
init = "1.0"
expr = "num_pols_if[t-1] * (1.0 - q_x[t-1])"

[[component]]
name = "claims"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "num_pols_if * q_x * sum_assured"

[[component]]
name = "reserve_seed"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "q_x[0] * (10.0 - 4.0)"

[[component]]
name = "pv_claims"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "npv(claims, discount_factor)"

[[component]]
name = "total_claims"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum(claims)"

[[component]]
name = "margin"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "pv_claims * (1.0 + 0.0)"
