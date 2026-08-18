format = "pir/1"

[run]
product = "product"
assumptions = "base"
modelpoints = "data/term.mpf.csv"
out = "runs/2026-06-30-base"
emit = "outputs"
retain = "ring"
on_trap = "abort"

[run.exec]
threads = 1
chunk_size = 1024

[[aggregation]]
name = "bel_by_gender"
group_by = ["gender"]
measure = "bel"
op = "sum"
