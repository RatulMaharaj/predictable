format = "pir/1"
module = "rate_units"

[timeline]
basis = "annual"
periods = 10
origin = "policy"
valuation_date = 2026-06-30
year_convention = "act/365"

[[modelpoint_field]]
name = "policy_number"
dtype = "str"
required = true
key = true

[[modelpoint_field]]
name = "sum_assured"
dtype = "f64"
unit = "money"
required = true

[[assumption]]
name = "annual_rate"
dtype = "f64"
unit = "rate(annual)"
shape = "Scalar"

[[assumption]]
name = "monthly_margin"
dtype = "f64"
unit = "rate(monthly)"
shape = "Scalar"

[[component]]
name = "loaded_rate"
kind = "Output"
dtype = "f64"
shape = "Scalar"
unit = "rate(annual)"
expr = "annual_rate + monthly_margin"
doc = "Rates on different bases cannot be added; convert with to_monthly / to_annual."
