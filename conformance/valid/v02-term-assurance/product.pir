format = "pir/1"

[product]
name = "TERM_UK"
modules = ["schema", "model"]
outputs = ["net_cashflow", "pv_premiums", "pv_claims", "pv_expenses", "bel", "reserve"]
key_field = "policy_number"
assumptions = "base"
doc = "UK level-term assurance, statutory valuation basis."
