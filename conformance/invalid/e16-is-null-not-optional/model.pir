format = "pir/1"
module = "presence"

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
name = "maybe_missing"
kind = "Output"
dtype = "bool"
shape = "PerMP"
unit = "none"
expr = "is_null(sum_assured)"
doc = "`sum_assured` is required, so it can never be missing and there is no presence bit."

[[component]]
name = "defaulted"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "coalesce(sum_assured, 0.0)"
doc = "coalesce is sugar for if is_null(x) then y else x and inherits the same rule."
