format = "pir/1"

[run]
product = "product"
assumptions = "base"
modelpoints = "data/minimal.mpf.csv"
out = "runs/minimal"
emit = "outputs"
retain = "ring"
storage_precision = "f64"
on_trap = "abort"
max_errors = 100
allow_table_drift = false
sum_kahan = false

[run.exec]
threads = 1
chunk_size = 1024
progress = false
