"""The comparison contract every engine returns.

An engine produces two dictionaries:

``per_mp``
    ``{component: array of shape (M,)}`` — the valuation results.
``series``
    ``{component: array of shape (M, T + 1)}`` — the projections, ``t = 0 .. T``.

Both are optional per component: an engine that does not implement a component simply
omits it, and the correctness gate compares the intersection and reports the shortfall
rather than passing silently. Collecting `series` is expensive at large `M`, so an engine
is asked for it only when the gate is running.
"""

from __future__ import annotations

from dataclasses import dataclass, field

import numpy as np


@dataclass
class Outputs:
    keys: list[str] = field(default_factory=list)
    per_mp: dict[str, np.ndarray] = field(default_factory=dict)
    series: dict[str, np.ndarray] = field(default_factory=dict)

    def component_names(self) -> set[str]:
        return set(self.per_mp) | set(self.series)
