format = "pir/1"
module = "enum_policy"

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
name = "gender"
dtype = "enum(Gender)"
required = true

[[table]]
name = "sa8990"
keys = [{ name = "gender", dtype = "enum(Gender)", policy = "clamp" }]
values = [{ name = "qx", dtype = "f64", unit = "prob" }]
on_missing = "error"
source = "tables/sa8990.csv"
digest = "sha256:29b744c8633098200a212d912cb497f34bc6b0cc01cb531473ae18eafc945694"

[[component]]
name = "qx"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "prob"
expr = "sa8990@(gender)"
