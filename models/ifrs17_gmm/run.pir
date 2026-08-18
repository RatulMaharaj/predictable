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
name = "csm_by_cohort"
group_by = ["cohort"]
measure = "csm_initial"
op = "sum"

[[aggregation]]
name = "insurance_service_result_by_cohort"
group_by = ["cohort"]
measure = "insurance_service_result"
op = "sum"
over_t = "each"
