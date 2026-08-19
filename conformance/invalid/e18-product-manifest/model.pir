format = "pir/1"
module = "model"
imports = ["shared"]

[[component]]
name = "bel"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum_assured / (1 + valuation_rate)"

[[component]]
name = "reserve"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "bel * 1.05"
