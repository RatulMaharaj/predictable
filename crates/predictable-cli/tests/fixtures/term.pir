format = "pir/1"
module = "term"

[timeline]
basis = "annual"
periods = 3
origin = "policy"
valuation_date = 2026-06-30

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

[[modelpoint_field]]
name = "premium"
dtype = "f64"
unit = "money"
required = true

[[modelpoint_field]]
name = "q"
dtype = "f64"
unit = "prob"
required = true

[[component]]
name = "survivors"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "count"
timing = "start"
init = "1.0"
expr = "survivors[t-1] * (1 - q)"

[[component]]
name = "claims"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "survivors * q * sum_assured"

[[component]]
name = "premium_income"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "survivors * premium"

[[component]]
name = "net_cashflow"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "claims - premium_income"

[[component]]
name = "bel"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum(net_cashflow)"
