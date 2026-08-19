format = "pir/1"

[product]
name = "TERM_MONTHLY"
modules = ["schema", "model"]
outputs = ["deaths", "surrenders", "premium_income", "death_claims", "renewal_expenses", "initial_expense", "net_cashflow", "pv_premiums", "pv_claims", "pv_expenses", "bel", "profit_margin", "reserve"]
key_field = "policy_number"
doc = "Level term assurance on a monthly basis — reference model 2 of 03-engine.md §11.2."
