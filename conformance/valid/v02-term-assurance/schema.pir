format = "pir/1"
module = "term_assurance"

[timeline]
basis = "annual"
periods = 40
origin = "policy"
valuation_date = 2026-06-30
year_convention = "act/365"

[[enum]]
name = "Gender"
values = ["M", "F"]

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
name = "gender"
dtype = "enum(Gender)"
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

[[assumption]]
name = "valuation_rate"
dtype = "f64"
unit = "rate(annual)"
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
name = "mortality_loading"
dtype = "f64"
unit = "factor"
shape = "Scalar"

[[table]]
name = "sa8990"
keys = [
  { name = "age", dtype = "i64", policy = "clamp" },
  { name = "gender", dtype = "enum(Gender)", policy = "exact" },
  { name = "smoker", dtype = "bool", policy = "exact" },
]
values = [{ name = "qx", dtype = "f64", unit = "prob" }]
on_missing = "error"
source = "tables/sa8990.csv"
digest = "sha256:9c0ebb1a86c5c5c84c377be552ff0fb704c8686a54be47ef04b86106b94baccb"

[[table]]
name = "lapse_rates"
keys = [{ name = "policy_year", dtype = "i64", policy = "step" }]
values = [{ name = "lapse_pa", dtype = "f64", unit = "prob" }]
on_missing = "default(0.0)"
source = "tables/lapses.csv"
digest = "sha256:086ef4fd70c037e2ea040ba21e5140c64cfc2c3f03be1ad504264d175de89f83"
