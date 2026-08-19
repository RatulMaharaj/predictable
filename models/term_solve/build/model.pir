format = "pir/1"
module = "model"
imports = ["schema"]

[[component]]
name = "age"
kind = "Derived"
dtype = "i64"
shape = "Series"
unit = "years"
timing = "start"
expr = "entry_age + t"
doc = "Attained age at the start of projection year `t`."

[[component]]
name = "in_term"
kind = "Derived"
dtype = "bool"
shape = "Series"
unit = "none"
timing = "start"
expr = "t < policy_term"
doc = "True while the policy is inside its contractual term."

[[component]]
name = "in_force_factor"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "factor"
timing = "start"
expr = "if in_term then 1.0 else 0.0"
doc = "1 inside the term, 0 after it. The single on/off switch every cashflow multiplies by."

[[component]]
name = "qx"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "prob"
timing = "end"
expr = "mortality@(age, sex, smoker) * mortality_loading"
doc = "Annual mortality rate: the table rate scaled by the valuation loading."

[[component]]
name = "wx"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "prob"
timing = "end"
expr = "lapses@(policy_year) * lapse_loading"
doc = "Annual lapse rate, stepped on policy year, scaled by the lapse loading."

[[component]]
name = "num_pols_if"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "count"
timing = "start"
init = "1.0"
expr = "num_pols_if[t-1] * (1 - qx[t-1]) * (1 - wx[t-1]) * (if in_term then 1.0 else 0.0)"
doc = "Policies in force at the start of year `t`, per policy sold."

[[component]]
name = "deaths"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "count"
timing = "end"
expr = "num_pols_if * qx * in_force_factor"
doc = "Expected deaths during year `t`."

[[component]]
name = "surrenders"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "count"
timing = "end"
expr = "num_pols_if * (1 - qx) * wx * in_force_factor"
doc = "Expected lapses during year `t`, on the lives that survived the year."

[[component]]
name = "expense_scale"
kind = "Derived"
dtype = "f64"
shape = "PerMP"
unit = "factor"
expr = "expenses@(expense_band)"
doc = "Per-policy expense scaling from the servicing band. Constant in `t`, so `PerMP`."

[[component]]
name = "premium_rate"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
init = "annual_premium"
expr = "premium_rate[t-1] * (1 + premium_escalation)"
doc = "Office premium payable in year `t`, escalating annually from the issue premium."

[[component]]
name = "premium_income"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "premium_rate * num_pols_if * in_force_factor"
doc = "Premium received in advance at the start of year `t`."

[[component]]
name = "death_claims"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "sum_assured * deaths"
doc = "Death benefit paid at the end of year `t`."

[[component]]
name = "renewal_expenses"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "renewal_expense_pa * compound(expense_inflation, t) * expense_scale * num_pols_if * in_force_factor"
doc = "Per-policy servicing expense, inflated from the valuation date and band-scaled."

[[component]]
name = "initial_expense"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "initial_expense_pct * annual_premium"
doc = "Acquisition expense, incurred once at issue as a percentage of the first premium."

[[component]]
name = "net_cashflow"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "mid"
expr = "retime(premium_income, mid) - retime(death_claims, mid) - retime(renewal_expenses, mid)"
doc = "Insurer-positive net flow, restated mid-year."

[[component]]
name = "disc_factor"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "factor"
timing = "point"
init = "1.0"
expr = "disc_factor[t-1] / (1 + valuation_rate)"
doc = "v^t at the flat valuation rate."

[[component]]
name = "pv_premiums"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "npv(premium_income, disc_factor)"
doc = "PV of future office premiums."

[[component]]
name = "pv_claims"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "npv(death_claims, disc_factor)"
doc = "PV of future death claims."

[[component]]
name = "pv_expenses"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "npv(renewal_expenses, disc_factor)"
doc = "PV of future renewal expenses."

[[component]]
name = "bel"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "pv_claims + pv_expenses + initial_expense - pv_premiums"
doc = "Best estimate liability at `t = 0` — the prospective gross premium reserve."

[[component]]
name = "profit_margin"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "none"
expr = "-bel / pv_premiums"
doc = "New-business margin: minus the BEL as a proportion of the PV of premiums."

[[component]]
name = "reserve"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "point"
init = "bel"
expr = "(reserve[t-1] + premium_income[t-1] - death_claims[t-1] - renewal_expenses[t-1]) * (1 + valuation_rate) * (if t <= policy_term then 1.0 else 0.0)"
doc = "Retrospective reserve roll-forward, seeded from the prospective `bel`."
