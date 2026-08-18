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

[[aggregation]]
name = "bel_by_sex"
group_by = ["sex"]
measure = "bel"
op = "sum"

[[aggregation]]
name = "net_cashflow_by_cohort"
group_by = ["cohort"]
measure = "net_cashflow"
op = "sum"
over_t = "each"
