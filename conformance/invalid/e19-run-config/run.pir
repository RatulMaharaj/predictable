format = "pir/1"

[run]
product = "product"
modelpoints = "data/mp.csv"
out = "runs/bad"
emit = "list"
emit_list = ["no_such_component"]
storage_precision = "f32"

[run.timeline]
periods = 60
