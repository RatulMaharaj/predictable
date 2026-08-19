format = "pir/1"
module = "lookup_arity"

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

[[enum]]
name = "Gender"
values = ["M", "F"]

[[modelpoint_field]]
name = "entry_age"
dtype = "i64"
unit = "years"
required = true

[[modelpoint_field]]
name = "gender"
dtype = "enum(Gender)"
required = true

[[table]]
name = "sa8990"
keys = [
  { name = "age", dtype = "i64", policy = "clamp" },
  { name = "gender", dtype = "enum(Gender)", policy = "exact" },
]
values = [{ name = "qx", dtype = "f64", unit = "prob" }]
on_missing = "error"
source = "tables/sa8990.csv"
digest = "sha256:bd2f3e49f961dc3a3877cd3ae7b3a26ec6f3a70e14e37d0024c34adaf48284bb"

[[component]]
name = "too_few_keys"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "prob"
expr = "sa8990@(entry_age)"

[[component]]
name = "wrong_key_dtype"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "prob"
expr = "sa8990@(gender, entry_age)"
