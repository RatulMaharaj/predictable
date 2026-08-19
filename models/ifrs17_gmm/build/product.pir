format = "pir/1"

[product]
name = "IFRS17_GMM"
modules = ["schema", "model"]
outputs = ["deaths", "surrenders", "maturities", "allocated_premium", "cost_of_insurance", "management_charge", "policy_fee", "guarantee_cost", "death_claims", "surrender_claims", "maturity_claims", "premium_income", "renewal_expenses", "initial_expense", "net_cashflow", "pv_premiums", "pv_death_claims", "pv_surrender_claims", "pv_maturity_claims", "pv_expenses", "pv_guarantee_cost", "bel", "account_at_maturity", "final_account_value", "profit_margin", "net_outflow", "risk_adjustment", "csm_initial", "loss_component_initial", "is_onerous", "coverage_units", "total_coverage_units", "csm_release", "ra_release", "fcf_balance", "lrc", "lrc_at_issue", "claims_incurred", "lic", "insurance_revenue", "insurance_service_expense", "insurance_service_result", "pv_csm_release", "pv_ra_release", "pv_insurance_service_result"]
key_field = "policy_number"
doc = "IFRS 17 general measurement model on a unit-linked endowment — reference model 4."
