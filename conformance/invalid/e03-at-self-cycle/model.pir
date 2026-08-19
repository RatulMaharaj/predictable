format = "pir/1"
module = "at_cycle"

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

[[component]]
name = "legal_seed"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "point"
init = "sum_assured"
expr = "legal_seed[0] * 1.05 + legal_seed[t-1] * 0.0"
doc = "Legal: the At edge is at t = 0, which is the seed period."

[[component]]
name = "illegal_seed"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "point"
init = "sum_assured"
expr = "illegal_seed[5] + sum_assured"
doc = "Illegal: at t = 5 this reads itself with no lag."
