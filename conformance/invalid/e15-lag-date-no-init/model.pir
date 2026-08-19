format = "pir/1"
module = "date_lag"

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
name = "prior_period_start"
kind = "Output"
dtype = "date"
shape = "Series"
unit = "none"
timing = "start"
expr = "prior_period_start[t-1]"
doc = "Out of range at t = 0, and date has no zero value, so an init is required."
