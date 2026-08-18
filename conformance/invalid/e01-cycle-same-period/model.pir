format = "pir/1"
module = "cycle"

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
name = "reserve"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "point"
expr = "bel * 1.05"

[[component]]
name = "bel"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "point"
expr = "reserve + sum_assured"
