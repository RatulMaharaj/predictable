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
expr = "entry_age + policy_year - 1"
doc = "Attained age."

[[component]]
name = "in_term"
kind = "Derived"
dtype = "bool"
shape = "Series"
unit = "none"
timing = "start"
expr = "t < policy_term * 12"
doc = "`policy_term` is in years; `t` is in months. The conversion is the whole point."

[[component]]
name = "in_force_factor"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "factor"
timing = "start"
expr = "if in_term then 1.0 else 0.0"

[[component]]
name = "qx_annual"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "prob"
timing = "end"
expr = "mortality@(age, sex, smoker) * mortality_loading"
doc = "The table's annual rate at the attained age."

[[component]]
name = "qx"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "prob"
timing = "end"
expr = "1 - pow(1 - qx_annual, 0.08333333333333333)"
doc = "Monthly mortality, on the *constant force* assumption within the policy year."

[[component]]
name = "wx"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "prob"
timing = "end"
expr = "1 - pow(1 - lapses@(policy_year) * lapse_loading, 0.08333333333333333)"
doc = "Monthly lapse rate, from the annual rate on the same constant-force basis."

[[component]]
name = "monthly_valuation_rate"
kind = "Derived"
dtype = "f64"
shape = "PerMP"
unit = "none"
expr = "pow(1 + valuation_rate, 0.08333333333333333) - 1"
doc = "`(1 + i)^(1/12) - 1`: the effective monthly rate equivalent to the annual basis."

[[component]]
name = "monthly_expense_inflation"
kind = "Derived"
dtype = "f64"
shape = "PerMP"
unit = "none"
expr = "pow(1 + expense_inflation, 0.08333333333333333) - 1"

[[component]]
name = "num_pols_if"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "count"
timing = "start"
init = "1.0"
expr = "num_pols_if[t-1] * (1 - qx[t-1]) * (1 - wx[t-1]) * (if in_term then 1.0 else 0.0)"
doc = "Policies in force at the start of month `t`, per policy sold."

[[component]]
name = "deaths"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "count"
timing = "end"
expr = "num_pols_if * qx * in_force_factor"

[[component]]
name = "surrenders"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "count"
timing = "end"
expr = "num_pols_if * (1 - qx) * wx * in_force_factor"

[[component]]
name = "expense_scale"
kind = "Derived"
dtype = "f64"
shape = "PerMP"
unit = "factor"
expr = "expenses@(expense_band)"

[[component]]
name = "monthly_premium"
kind = "Derived"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "annual_premium / 12"
doc = "Premiums are payable monthly in advance at one twelfth of the office premium."

[[component]]
name = "premium_income"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "monthly_premium * num_pols_if * in_force_factor"

[[component]]
name = "death_claims"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "sum_assured * deaths"

[[component]]
name = "renewal_expenses"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "renewal_expense_pa / 12 * pow(1 + monthly_expense_inflation, t) * expense_scale * num_pols_if * in_force_factor"
doc = "One twelfth of the annual per-policy expense, inflated monthly."

[[component]]
name = "initial_expense"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "initial_expense_pct * annual_premium"

[[component]]
name = "net_cashflow"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "mid"
expr = "retime(premium_income, mid) - retime(death_claims, mid) - retime(renewal_expenses, mid)"

[[component]]
name = "disc_factor"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "factor"
timing = "point"
init = "1.0"
expr = "disc_factor[t-1] / (1 + monthly_valuation_rate)"
doc = "v^t at the monthly-equivalent rate."

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
expr = "pv_claims + pv_expenses + initial_expense - pv_premiums"

[[component]]
name = "profit_margin"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "none"
expr = "-bel / pv_premiums"

[[component]]
name = "reserve"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "point"
init = "bel"
expr = "(reserve[t-1] + premium_income[t-1] - death_claims[t-1] - renewal_expenses[t-1]) * (1 + monthly_valuation_rate) * (if t <= policy_term * 12 then 1.0 else 0.0)"
doc = "Monthly retrospective roll-forward, seeded from `bel` exactly as in `term_annual`."
