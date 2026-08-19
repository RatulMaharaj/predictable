format = "pir/1"

[product]
name = "BROKEN"
modules = ["model"]
outputs = ["bel"]
key_field = "policy_number"
doc = "`shared` is imported by `model` but is not in `modules` (E0105), and `reserve` is an Output that the manifest omits (E0107)."
