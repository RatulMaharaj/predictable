format = "pir/1"
module = "over_twelve"

[timeline]
basis = "monthly"
periods = 120
origin = "policy"
valuation_date = 2026-06-30
year_convention = "act/365"

[[modelpoint_field]]
name = "policy_number"
dtype = "str"
required = true
key = true

[[assumption]]
name = "valuation_rate"
dtype = "f64"
unit = "rate(annual)"
shape = "Scalar"

[[component]]
name = "monthly_rate"
kind = "Output"
dtype = "f64"
shape = "Scalar"
unit = "rate(monthly)"
expr = "valuation_rate / 12"
doc = "Prophet models are full of this. The fix is to_monthly(r), or nominal_to_periodic(r, 12) if simple division is genuinely wanted."
