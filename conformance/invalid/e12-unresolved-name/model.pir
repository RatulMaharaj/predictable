format = "pir/1"
module = "unresolved"

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
name = "valuation_rate"
dtype = "f64"
unit = "rate(annual)"
shape = "Scalar"

[[component]]
name = "disc"
kind = "Output"
dtype = "f64"
shape = "Scalar"
unit = "factor"
expr = "v_from_i(valuation_rat)"
doc = "Levenshtein suggestion restricted to the assumption namespace: valuation_rate."
