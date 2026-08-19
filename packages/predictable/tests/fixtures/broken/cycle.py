"""A model whose only fault is a cycle — something no single Python body can see.

`a` needs `b` at the same `t`, and `b` needs `a`. Each function is fine on its own, so the
tracer cannot refuse it; the engine's checker raises `E0201` against the emitted `.pir`, and
`predictable build` has to show it here, in Python.
"""

from predictable import ModelPoint, key, series, timeline
from predictable.timing import END
from predictable.units import Money, Years

timeline(basis="annual", periods=5, origin="policy")


class MP(ModelPoint):
    policy_number: str = key()
    entry_age: Years
    premium: Money


@series(timing=END)
def a(b: Money, premium: Money) -> Money:
    return b + premium


@series(timing=END, output=True)
def b(a: Money) -> Money:
    return a * 2.0
