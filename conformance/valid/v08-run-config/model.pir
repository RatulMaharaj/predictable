format = "pir/1"
module = "solved"

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
name = "product_code"
dtype = "str"
required = true

[[modelpoint_field]]
name = "gender"
dtype = "enum(Gender)"
required = true

[[modelpoint_field]]
name = "sum_assured"
dtype = "f64"
unit = "money"
required = true

[[modelpoint_field]]
name = "annual_premium"
dtype = "f64"
unit = "money"
required = true

[[modelpoint_field]]
name = "in_force"
dtype = "bool"
required = true

[[assumption]]
name = "valuation_rate"
dtype = "f64"
unit = "rate(annual)"
shape = "Scalar"

[[assumption]]
name = "qx_flat"
dtype = "f64"
unit = "prob"
shape = "Scalar"

[[component]]
name = "in_force_at_val"
kind = "Derived"
dtype = "bool"
shape = "PerMP"
unit = "none"
expr = "in_force"
doc = "A bool PerMP component, so it is legal as an [[aggregation]] filter (IR §8.3)."

[[component]]
name = "num_pols_if"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "count"
timing = "start"
init = "1.0"
expr = "num_pols_if[t-1] * (1 - qx_flat)"

[[component]]
name = "disc_factor"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "factor"
timing = "point"
init = "1.0"
expr = "disc_factor[t-1] / (1 + valuation_rate)"

[[component]]
name = "premium_income"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "annual_premium * num_pols_if"

[[component]]
name = "death_claims"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "sum_assured * num_pols_if * qx_flat"

[[component]]
name = "bel"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "npv(death_claims, disc_factor) - npv(premium_income, disc_factor)"
doc = "The [[solve]] target: vary annual_premium until this is zero."
