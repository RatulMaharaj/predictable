format = "pir/1"
module = "init_cycle"

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
name = "reserve_a"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "point"
init = "seed_b"
expr = "reserve_a[t-1] * 1.05"

[[component]]
name = "seed_a"
kind = "Derived"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum(reserve_a)"

[[component]]
name = "reserve_b"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "point"
init = "seed_a"
expr = "reserve_b[t-1] * 1.03"

[[component]]
name = "seed_b"
kind = "Derived"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum(reserve_b)"
