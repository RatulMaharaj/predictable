format = "pir/1"
module = "fmt_case"

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
expr = "(sum_assured)"
name = "face"
unit = "money"
dtype = "f64"
kind = "Derived"
shape = "PerMP"
doc = "Redundant parentheses around an atom are removed."

[[component]]
name = "escalated"
shape = "Series"
timing = "start"
kind = "Derived"
dtype = "f64"
init = "face"
unit = "money"
expr = "escalated[t-1]*(1.050 + 0.000)"
doc = "Keys are reordered to name, kind, dtype, shape, unit, timing, init, expr, doc; the operator is spaced; 1.050 shortens to 1.05."

[[component]]
name = "interest"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "( escalated  *  rate ) + 0.10000000000000001"
doc = "Parentheses that disambiguate mixed * and + are preserved as written."

[[component]]
name = "total"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum( interest )"
