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
doc = "Attained age; steps on the policy anniversary, not every month."

[[component]]
name = "term_months"
kind = "Derived"
dtype = "f64"
shape = "PerMP"
unit = "none"
expr = "policy_term * 12"
doc = "The contract term in projection periods."

[[component]]
name = "in_term"
kind = "Derived"
dtype = "bool"
shape = "Series"
unit = "none"
timing = "start"
expr = "t < term_months"
doc = "True for `t = 0 .. term_months - 1` — the months a premium is actually paid."

[[component]]
name = "in_force_factor"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "factor"
timing = "start"
expr = "if in_term then 1.0 else 0.0"

[[component]]
name = "is_maturity"
kind = "Derived"
dtype = "bool"
shape = "Series"
unit = "none"
timing = "end"
expr = "t == term_months - 1"
doc = "The single month in which the contract matures."

[[component]]
name = "qx_annual"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "prob"
timing = "end"
expr = "mortality@(age, sex, smoker) * mortality_loading"

[[component]]
name = "qx"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "prob"
timing = "end"
expr = "1 - pow(1 - qx_annual, 0.08333333333333333)"
doc = "Monthly mortality on the constant-force assumption."

[[component]]
name = "wx_annual"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "prob"
timing = "end"
expr = "lapses@(policy_year) * lapse_loading"

[[component]]
name = "wx"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "prob"
timing = "end"
expr = "if is_maturity then 0.0 else 1 - pow(1 - wx_annual, 0.08333333333333333)"
doc = "Monthly surrender rate. Nobody surrenders in the maturity month."

[[component]]
name = "spot_rate"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "rate(annual)"
timing = "start"
expr = "yield_curve@(policy_year)"
doc = "The annually compounded spot rate for a cashflow at the end of policy year `y`."

[[component]]
name = "credit_rate_m"
kind = "Derived"
dtype = "f64"
shape = "PerMP"
unit = "none"
expr = "pow(1 + credit_rate, 0.08333333333333333) - 1"
doc = "Monthly equivalent of the annual investment return credited to the account."

[[component]]
name = "amc_m"
kind = "Derived"
dtype = "f64"
shape = "PerMP"
unit = "none"
expr = "1 - pow(1 - amc_pa, 0.08333333333333333)"
doc = "Monthly equivalent of the annual management charge."

[[component]]
name = "expense_scale"
kind = "Derived"
dtype = "f64"
shape = "PerMP"
unit = "factor"
expr = "expenses@(expense_band)"

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
name = "maturities"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "count"
timing = "end"
expr = "num_pols_if * (1 - qx) * (1 - wx) * (if is_maturity then 1.0 else 0.0)"
doc = "Everyone still standing at the end of the maturity month matures."

[[component]]
name = "alloc_rate"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "factor"
timing = "start"
expr = "if policy_year == 1 then alloc_rate_year1 else alloc_rate_renewal"
doc = "Premium allocation rate: reduced in policy year 1 to recover acquisition costs."

[[component]]
name = "allocated_premium"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "monthly_premium * alloc_rate * in_force_factor"
doc = "The part of the premium that reaches the account, per policy in force."

[[component]]
name = "account_value"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "point"
init = "initial_account"
expr = "(account_value[t-1] + allocated_premium[t-1] - policy_charges[t-1]) * (1 + credit_rate_m) * in_force_factor"
doc = "Account value per policy in force, at the start of month `t`."

[[component]]
name = "sum_at_risk"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "max(sum_assured - account_value, 0.0)"
doc = "`max(SA - AV, 0)`: the part of the death benefit the insurer actually carries."

[[component]]
name = "cost_of_insurance"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "sum_at_risk * qx * coi_loading"
doc = "Monthly mortality charge on the sum at risk."

[[component]]
name = "management_charge"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "account_value * amc_m"
doc = "Annual management charge, taken monthly on the account value."

[[component]]
name = "policy_fee"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "policy_fee_pm * pow(1 + expense_inflation, policy_year - 1)"
doc = "Flat monthly fee, escalating on each policy anniversary rather than each month."

[[component]]
name = "policy_charges"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "(cost_of_insurance + management_charge + policy_fee) * in_force_factor"
doc = "Everything deducted from the account in month `t`."

[[component]]
name = "premiums_paid"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "point"
init = "monthly_premium"
expr = "premiums_paid[t-1] + monthly_premium * in_force_factor"
doc = "Cumulative premiums paid to the start of month `t`. The guarantee is defined on it."

[[component]]
name = "death_benefit"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "max(sum_assured, account_value)"
doc = "`max(SA, AV)` per policy: the account never buys less than the sum assured."

[[component]]
name = "surrender_benefit"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "max(account_value * (1 - surrender_penalty@(policy_year)), 0.0)"
doc = "Account value less the surrender penalty, floored at zero."

[[component]]
name = "guaranteed_maturity"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "premiums_paid * guarantee_pct"
doc = "The maturity floor: a stated proportion of the premiums actually paid."

[[component]]
name = "maturity_benefit"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "max(account_value, guaranteed_maturity)"
doc = "`max(AV, guarantee)` — the guarantee bites only when the fund underperforms."

[[component]]
name = "guarantee_cost"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "(maturity_benefit - retime(account_value, end)) * maturities"
doc = "The part of the maturity payment the guarantee, not the fund, is paying for."

[[component]]
name = "death_claims"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "death_benefit * deaths"

[[component]]
name = "surrender_claims"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "surrender_benefit * surrenders"

[[component]]
name = "maturity_claims"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "maturity_benefit * maturities"

[[component]]
name = "premium_income"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "monthly_premium * num_pols_if * in_force_factor"

[[component]]
name = "renewal_expenses"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "renewal_expense_pa / 12 * pow(1 + expense_inflation, policy_year - 1) * expense_scale * num_pols_if * in_force_factor"

[[component]]
name = "initial_expense"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "initial_expense_pct * monthly_premium * 12"

[[component]]
name = "net_cashflow"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "mid"
expr = "retime(premium_income, mid) - retime(death_claims, mid) - retime(surrender_claims, mid) - retime(maturity_claims, mid) - retime(renewal_expenses, mid)"
doc = "Insurer-positive net flow, all legs restated to mid-month."

[[component]]
name = "disc_factor"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "factor"
timing = "point"
expr = "pow(1 + spot_rate, 0 - t / 12.0)"
doc = "`v(t) = (1 + s_y)^(-t/12)` off the spot curve, not a flat rate."

[[component]]
name = "pv_premiums"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "npv(premium_income, disc_factor)"

[[component]]
name = "pv_death_claims"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "npv(death_claims, disc_factor)"

[[component]]
name = "pv_surrender_claims"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "npv(surrender_claims, disc_factor)"

[[component]]
name = "pv_maturity_claims"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "npv(maturity_claims, disc_factor)"

[[component]]
name = "pv_expenses"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "npv(renewal_expenses, disc_factor)"

[[component]]
name = "pv_guarantee_cost"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "npv(guarantee_cost, disc_factor)"
doc = "The cost of the maturity guarantee, on the central assumption set."

[[component]]
name = "bel"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "pv_death_claims + pv_surrender_claims + pv_maturity_claims + pv_expenses + initial_expense - pv_premiums"
doc = "Best estimate liability at issue."

[[component]]
name = "account_at_maturity"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "retime(account_value, end) * maturities"
doc = "The account value carried into the maturity payment, zero in every other month."

[[component]]
name = "final_account_value"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum(account_at_maturity)"
doc = "The account value at maturity, as a per-policy check figure."

[[component]]
name = "profit_margin"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "none"
expr = "-bel / pv_premiums"
