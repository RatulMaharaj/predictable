format = "pir/1"

[product]
name = "SOLVED"
modules = ["model"]
outputs = ["num_pols_if", "bel"]
key_field = "policy_number"
assumptions = "base"
doc = "`outputs` is a validated manifest, not a selector: it must equal the set of kind = Output components (IR §8.4.1)."
