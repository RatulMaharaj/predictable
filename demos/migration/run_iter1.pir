format = "pir/1"

[run]
product = "build_iter1"
assumptions = "base.pir"
modelpoints = "data/modelpoints.csv"
out = "runs/iter1"
emit = "all"
retain = "full"
on_trap = "abort"

[run.exec]
threads = 1
chunk_size = 1024
