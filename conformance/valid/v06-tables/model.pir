format = "pir/1"
module = "tables"

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
name = "smoker"
dtype = "bool"
required = true

[[modelpoint_field]]
name = "expense_band"
dtype = "i64"
required = true

# FsResolver, relative to this .pir file's directory. clamp + exact keys, hard miss.
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
digest = "sha256:eb2924ee6f2343aa5e222fba4e2b33e71ec12b76f300007a3fed51c4691f7794"

# FsResolver, interpolating key and interpolating miss policy.
[[table]]
name = "yield_curve"
keys = [{ name = "term", dtype = "i64", policy = "interpolate" }]
values = [{ name = "spot", dtype = "f64", unit = "rate(annual)" }]
on_missing = "interpolate(term)"
source = "tables/yield_curve.csv"
digest = "sha256:ee308849cfe4570bb59d210d2a09d1b20248a4ad7672c42ddda543b2d3e9fce9"

# MapResolver: the host supplies the bytes under this name. No filesystem.
[[table]]
name = "expense_scale"
keys = [{ name = "band", dtype = "i64", policy = "step" }]
values = [{ name = "scale", dtype = "f64", unit = "factor" }]
on_missing = "default(1.0)"
source = "resource:expense_scale_2026"
digest = "sha256:5c1d0e9f8a7b6c5d4e3f2a1b0c9d8e7f6a5b4c3d2e1f0a9b8c7d6e5f4a3b2c1d"

# InlineResolver: rows carried in the module, in keys ++ values order, sorted by key.
[[table]]
name = "lapse_rates"
keys = [{ name = "policy_year", dtype = "i64", policy = "step" }]
values = [{ name = "lapse_pa", dtype = "f64", unit = "prob" }]
on_missing = "default(0.0)"
source = "inline"
digest = "sha256:31aa7e0c9b2f45d8e6103c7fa8b4d92e0517c6ab3f8e1d40b95a2c7e6d80f1a3"
rows = [
  [1, 0.12],
  [2, 0.08],
  [3, 0.05],
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
expr = "sa8990@(age, gender, smoker)"
doc = "clamp on age: ages outside the table's range clamp to its endpoints rather than missing."

[[component]]
name = "spot"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "rate(annual)"
timing = "point"
expr = "yield_curve@(t + 1)"
doc = "A key expression may be any Expr; interpolation applies between tabulated terms."

[[component]]
name = "expense_factor"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "factor"
expr = "expense_scale@(expense_band)"

[[component]]
name = "wx"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "prob"
timing = "end"
expr = "lapse_rates@(policy_year)"
doc = "Beyond the last tabulated policy year the step policy holds the last value; a key below the first row misses and takes default(0.0)."
