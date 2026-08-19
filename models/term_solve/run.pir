format = "pir/1"

[run]
product = "build"
assumptions = "base.pir"
modelpoints = "data/modelpoints.csv"
out = "runs/base"
emit = "outputs"
retain = "ring"
on_trap = "abort"

[run.exec]
threads = 1
chunk_size = 1024

[[solve]]
name = "breakeven_premium"
target = "bel"
to = 0.0
vary = "annual_premium"
scope = "per_mp"
tolerance = 1e-8
max_iter = 60
method = "brent"
bracket = [1.0, 100000.0]

[[aggregation]]
name = "bel_by_sex"
group_by = ["sex"]
measure = "bel"
op = "sum"
