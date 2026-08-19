format = "pir/1"

# The IFRS 17 GMM valuation of the synthetic in-force portfolio at 30 June 2026.
#
# The model is the committed reference model, unmodified: `models/ifrs17_gmm/build`. Only
# the portfolio and the aggregations are this demo's. That is deliberate — a sample
# valuation whose model has been tweaked for the sample is not a sample of anything.

[run]
product = "../../models/ifrs17_gmm/build"
assumptions = "../../models/ifrs17_gmm/base.pir"
modelpoints = "data/modelpoints.csv"
out = "runs/valuation"
emit = "outputs"
retain = "full"
on_trap = "abort"

[run.exec]
threads = 4
chunk_size = 1024

# ---------------------------------------------------------------------------------------
# The IFRS 17 disclosure cuts. Groupings do not nest (IR §8.3): drill-down is a prefix of
# the ordered key tuple, so `["cohort", "expense_band"]` also answers the `cohort` question.
# ---------------------------------------------------------------------------------------

[[aggregation]]
name = "csm_initial_by_cohort"
group_by = ["cohort"]
measure = "csm_initial"
op = "sum"

[[aggregation]]
name = "loss_component_by_cohort"
group_by = ["cohort"]
measure = "loss_component_initial"
op = "sum"

[[aggregation]]
name = "risk_adjustment_by_cohort"
group_by = ["cohort"]
measure = "risk_adjustment"
op = "sum"

[[aggregation]]
name = "onerous_contracts_by_cohort"
group_by = ["cohort"]
measure = "is_onerous"
op = "sum"

[[aggregation]]
name = "csm_release_by_cohort"
group_by = ["cohort"]
measure = "csm_release"
op = "sum"
over_t = "each"

[[aggregation]]
name = "ra_release_by_cohort"
group_by = ["cohort"]
measure = "ra_release"
op = "sum"
over_t = "each"

[[aggregation]]
name = "insurance_revenue_by_cohort"
group_by = ["cohort"]
measure = "insurance_revenue"
op = "sum"
over_t = "each"

[[aggregation]]
name = "insurance_service_result_by_cohort"
group_by = ["cohort"]
measure = "insurance_service_result"
op = "sum"
over_t = "each"

[[aggregation]]
name = "lrc_by_cohort"
group_by = ["cohort"]
measure = "lrc"
op = "sum"
over_t = "each"

[[aggregation]]
name = "lic_by_cohort"
group_by = ["cohort"]
measure = "lic"
op = "sum"
over_t = "each"

# `csm`, `ra_balance` and `fcf_balance` are the roll-forward's own balances. `csm` and
# `ra_balance` are internal to the model rather than declared outputs, so they are picked up
# here as aggregations: the roll-forward needs the opening and closing balance of each
# period, and an aggregation reads the component whether or not the run writes it per
# model point.

[[aggregation]]
name = "csm_by_cohort"
group_by = ["cohort"]
measure = "csm"
op = "sum"
over_t = "each"

[[aggregation]]
name = "ra_balance_by_cohort"
group_by = ["cohort"]
measure = "ra_balance"
op = "sum"
over_t = "each"

[[aggregation]]
name = "fcf_balance_by_cohort"
group_by = ["cohort"]
measure = "fcf_balance"
op = "sum"
over_t = "each"
