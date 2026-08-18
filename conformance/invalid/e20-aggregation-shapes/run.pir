format = "pir/1"

[run]
product = "product"
modelpoints = "data/mp.csv"
out = "runs/aggs"
emit = "outputs"

[[aggregation]]
name = "by_series_key"
group_by = ["policy_year_series"]
measure = "bel"
op = "sum"

[[aggregation]]
name = "by_float_key"
group_by = ["sum_assured"]
measure = "bel"
op = "sum"

[[aggregation]]
name = "permp_over_t"
group_by = ["policy_number"]
measure = "bel"
op = "sum"
over_t = "each"
