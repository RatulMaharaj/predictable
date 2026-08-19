format = "pir/1"
module = "term_assurance"
imports = ["schema"]

[[component]]
name = "age"
kind = "Derived"
dtype = "i64"
shape = "Series"
unit = "years"
timing = "start"
expr = "entry_age + t"
doc = "Attained age at the start of projection year t."

[[component]]
name = "in_term"
kind = "Derived"
dtype = "bool"
shape = "Series"
unit = "none"
timing = "start"
expr = "t < policy_term"
doc = "True while the policy is within its contractual term."

[[component]]
name = "qx"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "prob"
timing = "end"
expr = "sa8990@(age, gender, smoker) * mortality_loading"
doc = "Annual mortality rate, table rate scaled by the valuation loading."

[[component]]
name = "wx"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "prob"
timing = "end"
expr = "lapse_rates@(policy_year)"
doc = "Annual lapse rate, stepped on policy year."

[[component]]
name = "num_pols_if"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "count"
timing = "start"
init = "1.0"
expr = "num_pols_if[t-1] * (1 - qx[t-1]) * (1 - wx[t-1]) * (if in_term then 1.0 else 0.0)"
doc = "Survivorship. Self-referential with lag 1 — legal per IR §3.1."

[[component]]
name = "deaths"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "count"
timing = "end"
expr = "num_pols_if * qx"

[[component]]
name = "premium_rate"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
init = "annual_premium"
expr = "premium_rate[t-1] * (1 + premium_escalation)"
doc = "Escalating premium, compounding annually from the issue premium."

[[component]]
name = "premium_income"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "premium_rate * num_pols_if * (if in_term then 1.0 else 0.0)"

[[component]]
name = "death_claims"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "sum_assured * deaths"

[[component]]
name = "renewal_expenses"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "renewal_expense_pa * compound(expense_inflation, t) * num_pols_if * (if in_term then 1.0 else 0.0)"

[[component]]
name = "net_cashflow"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "mid"
expr = "retime(premium_income, mid) - retime(death_claims, mid) - retime(renewal_expenses, mid)"
doc = "Insurer-positive net flow. Explicit retiming so the sign convention is auditable."

[[component]]
name = "disc_factor"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "factor"
timing = "point"
init = "1.0"
expr = "disc_factor[t-1] / (1 + valuation_rate)"
doc = "v^t. Recursive rather than pow() so a term-structure rate is a one-line change."

[[component]]
name = "pv_premiums"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "npv(premium_income, disc_factor)"

[[component]]
name = "pv_claims"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "npv(death_claims, disc_factor)"

[[component]]
name = "pv_expenses"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "npv(renewal_expenses, disc_factor)"

[[component]]
name = "bel"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "pv_claims + pv_expenses - pv_premiums"
doc = "Best estimate liability, prospective net premium reserve at t = 0."

[[component]]
name = "reserve"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "point"
init = "bel"
expr = "(reserve[t-1] + premium_income[t-1] - death_claims[t-1] - renewal_expenses[t-1]) * (1 + valuation_rate)"
doc = "Retrospective roll-forward, seeded from the stage-2 npv `bel` through `init` — the one legal backward channel (IR §8.2)."
