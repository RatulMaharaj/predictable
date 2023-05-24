import predictable as pr

df = pr.read_rpt("./examples/input/modelpoints.rpt")
df["premium"] = 200

df.predictable.to_rpt("./examples/output/modelpoints_modified.rpt")
