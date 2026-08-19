format = "pir/1"

[run]
product = "product"
assumptions = "base"
modelpoints = "data/solved.mpf.parquet"
out = "runs/2026-06-30-solve"
emit = "list"
emit_list = ["disc_factor"]
retain = "full"
storage_precision = "f64"
on_trap = "continue"
max_errors = 100
allow_table_drift = false
sum_kahan = false

[run.exec]
threads = 8
chunk_size = 1024
progress = true

[[solve]]
name = "premium_solve"
target = "bel"
to = 0.0
vary = "annual_premium"
scope = "per_mp"
tolerance = 1e-8
max_iter = 50
method = "brent"
bracket = [0.0, 1000000.0]

[[aggregation]]
name = "bel_by_product"
group_by = ["product_code"]
measure = "bel"
op = "sum"
filter = "in_force_at_val"

[[aggregation]]
name = "bel_by_product_gender"
group_by = ["product_code", "gender"]
measure = "bel"
op = "sum"
filter = "in_force_at_val"

[[aggregation]]
name = "pols_by_gender_each_t"
group_by = ["gender"]
measure = "num_pols_if"
op = "sum"
over_t = "each"
