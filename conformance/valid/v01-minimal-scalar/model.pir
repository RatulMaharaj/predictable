format = "pir/1"
module = "minimal"

[timeline]
basis = "annual"
periods = 1
origin = "policy"
valuation_date = 2026-06-30
year_convention = "act/365"

[[modelpoint_field]]
name = "policy_number"
dtype = "str"
required = true
key = true

[[assumption]]
name = "valuation_rate"
dtype = "f64"
unit = "rate(annual)"
shape = "Scalar"

[[component]]
name = "one_year_discount"
kind = "Output"
dtype = "f64"
shape = "Scalar"
unit = "factor"
expr = "v_from_i(valuation_rate)"
doc = "v = 1 / (1 + i). A Scalar output: one value for the whole run."
