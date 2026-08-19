format = "pir/1"

[product]
name = "TERM_ANNUAL"
modules = ["schema", "model"]
outputs = ["death_claims", "pv_claims"]
key_field = "policy_number"
doc = "The fixture product of the predictable build tests."
