format = "pir/1"
module = "lookup_miss"

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

[[modelpoint_field]]
name = "entry_age"
dtype = "i64"
unit = "years"
required = true

[[table]]
name = "qx_table"
keys = [{ name = "age", dtype = "i64", policy = "exact" }]
values = [{ name = "qx", dtype = "f64", unit = "prob" }]
on_missing = "error"
source = "inline"
digest = "sha256:0a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f9"
rows = [
  [40, 0.00135],
  [41, 0.00147],
  [42, 0.00161],
]

[[component]]
name = "age"
kind = "Derived"
dtype = "i64"
shape = "Series"
unit = "years"
timing = "start"
expr = "entry_age + t"

[[component]]
name = "qx"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "prob"
timing = "end"
expr = "qx_table@(age)"
doc = "policy = exact and on_missing = error, so age 43 at t = 3 traps."
