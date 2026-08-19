format = "pir/1"
module = "escape"

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

[[table]]
name = "escaping"
keys = [{ name = "band", dtype = "i64", policy = "step" }]
values = [{ name = "load", dtype = "f64", unit = "factor" }]
on_missing = "error"
source = "../../../../etc/passwd"
digest = "sha256:1111111111111111111111111111111111111111111111111111111111111111"

[[table]]
name = "absolute"
keys = [{ name = "band", dtype = "i64", policy = "step" }]
values = [{ name = "load", dtype = "f64", unit = "factor" }]
on_missing = "error"
source = "/etc/passwd"
digest = "sha256:2222222222222222222222222222222222222222222222222222222222222222"

[[component]]
name = "load"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "factor"
expr = "escaping@(1) + absolute@(1)"
