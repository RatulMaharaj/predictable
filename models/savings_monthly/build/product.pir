format = "pir/1"

[product]
name = "SAVINGS_MONTHLY"
modules = ["schema", "model"]
outputs = ["deaths", "surrenders", "maturities", "allocated_premium", "cost_of_insurance", "management_charge", "policy_fee", "guarantee_cost", "death_claims", "surrender_claims", "maturity_claims", "premium_income", "renewal_expenses", "initial_expense", "net_cashflow", "pv_premiums", "pv_death_claims", "pv_surrender_claims", "pv_maturity_claims", "pv_expenses", "pv_guarantee_cost", "bel", "account_at_maturity", "final_account_value", "profit_margin"]
key_field = "policy_number"
doc = "Unit-linked endowment, monthly — reference model 3 of 03-engine.md §11.2."
