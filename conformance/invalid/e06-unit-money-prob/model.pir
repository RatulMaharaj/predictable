format = "pir/1"
module = "money_units"

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
name = "qx_flat"
dtype = "f64"
unit = "prob"
shape = "Scalar"

[[component]]
name = "legal_expected_claim"
kind = "Derived"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum_assured * qx_flat"
doc = "Legal: money * prob = money."

[[component]]
name = "bad_sum"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum_assured + qx_flat"

[[component]]
name = "bad_product"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum_assured * sum_assured"
