format = "pir/1"
module = "aggs"

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
name = "policy_year_series"
kind = "Derived"
dtype = "i64"
shape = "Series"
unit = "none"
timing = "start"
expr = "t + 1"

[[component]]
name = "bel"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum_assured * 0.1"
