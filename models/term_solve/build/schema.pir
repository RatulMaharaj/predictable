format = "pir/1"
module = "schema"

[timeline]
basis = "annual"
periods = 40
origin = "policy"
valuation_date = 2026-06-30
year_convention = "act/365"

[[modelpoint_field]]
name = "policy_number"
dtype = "str"
required = true
key = true

[[modelpoint_field]]
name = "entry_age"
dtype = "i64"
unit = "years"
required = true

[[modelpoint_field]]
name = "sex"
dtype = "str"
required = true

[[modelpoint_field]]
name = "smoker"
dtype = "bool"
required = true

[[modelpoint_field]]
name = "sum_assured"
dtype = "f64"
unit = "money"
required = true

[[modelpoint_field]]
name = "annual_premium"
dtype = "f64"
unit = "money"
required = true

[[modelpoint_field]]
name = "policy_term"
dtype = "i64"
unit = "years"
required = true

[[modelpoint_field]]
name = "expense_band"
dtype = "i64"
required = true

[[assumption]]
name = "valuation_rate"
dtype = "f64"
unit = "rate(annual)"
shape = "Scalar"

[[assumption]]
name = "mortality_loading"
dtype = "f64"
unit = "factor"
shape = "Scalar"

[[assumption]]
name = "lapse_loading"
dtype = "f64"
unit = "factor"
shape = "Scalar"

[[assumption]]
name = "premium_escalation"
dtype = "f64"
unit = "rate(annual)"
shape = "Scalar"

[[assumption]]
name = "expense_inflation"
dtype = "f64"
unit = "rate(annual)"
shape = "Scalar"

[[assumption]]
name = "renewal_expense_pa"
dtype = "f64"
unit = "money"
shape = "Scalar"

[[assumption]]
name = "initial_expense_pct"
dtype = "f64"
unit = "factor"
shape = "Scalar"

[[table]]
name = "mortality"
keys = [
  { name = "age", dtype = "i64", policy = "clamp" },
  { name = "sex", dtype = "str", policy = "exact" },
  { name = "smoker", dtype = "bool", policy = "exact" },
]
values = [{ name = "qx", dtype = "f64", unit = "prob" }]
on_missing = "error"
source = "tables/mortality.csv"
digest = "sha256:de972761d11068b2c7dcadfaab1edf4b3e81ed4d4374f5f35fd499485d363cf6"

[[table]]
name = "lapses"
keys = [{ name = "policy_year", dtype = "i64", policy = "step" }]
values = [{ name = "lapse_pa", dtype = "f64", unit = "prob" }]
on_missing = "error"
source = "tables/lapses.csv"
digest = "sha256:42f8f7bcfcc324a9d609f731a9ef89c6c2725bb8ad0a666d819e4fbb8c37da81"

[[table]]
name = "expenses"
keys = [{ name = "band", dtype = "i64", policy = "step" }]
values = [{ name = "scale", dtype = "f64", unit = "factor" }]
on_missing = "error"
source = "tables/expenses.csv"
digest = "sha256:d8df4dfd42391238a6ba7f29ddf7d65a82aae4eab5acc9c13eed1f21a4f332a7"
