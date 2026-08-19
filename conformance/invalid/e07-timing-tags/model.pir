format = "pir/1"
module = "timing"

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

[[component]]
name = "timed_permp"
kind = "Derived"
dtype = "f64"
shape = "PerMP"
unit = "money"
timing = "end"
expr = "sum_assured * 0.5"
doc = "timing is only for shape = Series; a PerMP has none."

[[component]]
name = "untimed_series"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
expr = "sum_assured * 0.01"
doc = "A Series must carry a timing tag; it is consumed by npv."

[[component]]
name = "unknown_tag"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "average"
expr = "untimed_series + timed_permp"
doc = "Timing is one of start | end | mid | point."
