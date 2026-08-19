format = "pir/1"
module = "schema"

[timeline]
basis = "annual"
periods = 10
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
name = "sum_assured"
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
name = "mortality_loading"
dtype = "f64"
unit = "factor"
shape = "Scalar"

[[table]]
name = "mortality"
keys = [{ name = "age", dtype = "i64", policy = "clamp" }]
values = [{ name = "qx", dtype = "f64", unit = "prob" }]
on_missing = "error"
source = "tables/mortality.csv"
digest = "sha256:f7a571513c88186e30dd65dde3ff79a2f6b9fceef7746e0a61783cccb319bd87"
