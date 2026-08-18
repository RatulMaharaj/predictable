format = "pir/1"
module = "b"
imports = ["schema"]

[[component]]
name = "loading"
kind = "Derived"
dtype = "f64"
shape = "PerMP"
unit = "factor"
expr = "1.1"

[[component]]
name = "loaded_sum"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum_assured * loading"
doc = "Which `loading`? Neither — shadowing across modules is an error, not a silent win."
