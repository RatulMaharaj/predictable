format = "pir/1"
module = "monthly"

[timeline]
basis = "monthly"
periods = 480
origin = "valuation"
valuation_date = 2026-06-30
year_convention = "act/365"

[[modelpoint_field]]
name = "policy_number"
dtype = "str"
required = true
key = true

[[modelpoint_field]]
name = "monthly_premium"
dtype = "f64"
unit = "money"
required = true

[[modelpoint_field]]
name = "issue_date"
dtype = "date"
required = true

[[assumption]]
name = "valuation_rate_pa"
dtype = "f64"
unit = "rate(annual)"
shape = "Scalar"

[[assumption]]
name = "nominal_rate_pa"
dtype = "f64"
unit = "rate(annual)"
shape = "Scalar"

[[assumption]]
name = "expense_pa"
dtype = "f64"
unit = "money"
shape = "Scalar"

[[component]]
name = "monthly_rate"
kind = "Derived"
dtype = "f64"
shape = "Scalar"
unit = "rate(monthly)"
expr = "to_monthly(valuation_rate_pa)"
doc = "Compounding conversion, (1 + r)^(1/12) - 1. IR §5 requires this to be explicit."

[[component]]
name = "nominal_monthly_rate"
kind = "Derived"
dtype = "f64"
shape = "Scalar"
unit = "rate(monthly)"
expr = "nominal_to_periodic(nominal_rate_pa, 12)"
doc = "Simple division, named so the choice is visible in the diff (IR §2.8)."

[[component]]
name = "implied_annual_rate"
kind = "Derived"
dtype = "f64"
shape = "Scalar"
unit = "rate(annual)"
expr = "to_annual(monthly_rate)"

[[component]]
name = "months_in_force"
kind = "Derived"
dtype = "i64"
shape = "Series"
unit = "months"
timing = "start"
expr = "months_between(issue_date, period_start_date)"
doc = "Uses the timeline input `period_start_date` and a modelpoint date field."

[[component]]
name = "policy_year_frac"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "years"
timing = "start"
expr = "year_frac(issue_date, period_start_date, \"act/365\")"

[[component]]
name = "anniversary_month"
kind = "Derived"
dtype = "bool"
shape = "Series"
unit = "none"
timing = "start"
expr = "is_anniversary and policy_month == 1"
doc = "Both `is_anniversary` and `policy_month` are Input.Timeline components (IR §5)."

[[component]]
name = "renewal_month"
kind = "Derived"
dtype = "bool"
shape = "Series"
unit = "none"
timing = "start"
expr = "month_of_year == month(issue_date)"

[[component]]
name = "premium_income"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "monthly_premium * (if months_in_force < 240 then 1.0 else 0.0)"

[[component]]
name = "expense_outgo"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "expense_pa * year_frac * (if months_in_force < 240 then 1.0 else 0.0)"
doc = "`year_frac` with no arguments is the timeline input: the length of period t in years."

[[component]]
name = "disc_factor"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "factor"
timing = "point"
init = "1.0"
expr = "disc_factor[t-1] / (1 + monthly_rate)"

[[component]]
name = "pv_net"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "npv(premium_income, disc_factor) - npv(expense_outgo, disc_factor)"
doc = "Both flows are `start`-timed, so npv uses v^t for each (IR §2.5)."
