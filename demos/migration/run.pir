format = "pir/1"

[run]
product = "build"
assumptions = "base.pir"
modelpoints = "data/modelpoints.csv"
out = "runs/base"
# `emit = "all"`, not `"outputs"`: "which component is the root" is partly a fact about
# what the run was asked to write down, and a migration wants the differ to be able to
# name `qx` rather than the nearest output downstream of it.
emit = "all"
retain = "full"
on_trap = "abort"

[run.exec]
threads = 1
chunk_size = 1024
