format = "pir/1"
module = "lints"

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
name = "rate"
dtype = "f64"
unit = "rate(annual)"
shape = "Scalar"

[[component]]
name = "never_used"
kind = "Derived"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum_assured * 2.0"
doc = "W0101: nothing reads this and it is not an Output."

[[component]]
name = "untagged_money"
kind = "Derived"
dtype = "f64"
shape = "PerMP"
unit = "none"
expr = "sum_assured + 100.0"
doc = "W0102: money-shaped arithmetic declared unit = none."

[[component]]
name = "premium"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "sum_assured * 0.01"

[[component]]
name = "claims"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "sum_assured * 0.002"

[[component]]
name = "mixed_timing"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "premium + claims"
doc = "W0103: a start flow added to an end flow. Fix with shift(claims, 1) or retime."

[[component]]
name = "constant_series"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "factor"
timing = "point"
expr = "1.0 + rate"
doc = "W0104: constant across modelpoints and time; this wants to be an assumption."

[[component]]
name = "pointless_retime"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "retime(sum(premium), end)"
doc = "W0105: sum() is a PerMP and has no timing, so retiming it changes nothing."
