"""The correctness gate — the part of the benchmark that runs first and can veto the rest.

`03-engine.md` §11.4: *every scenario × size asserts predictable ≡ cashflower ≡ lifelib
before any timing is published. Where they disagree, the benchmark fails and the
discrepancy is written up — a disagreement is more interesting than a speedup.*

The metric
----------
For each component, the error is the largest absolute difference across all compared
cells, divided by the *component's own scale* — the largest absolute value the reference
takes across those same cells:

    err(component) = max|a − b| / max(max|b|, floor)

Scale-relative rather than cell-relative, because a cell-by-cell relative error is
meaningless where the reference is a rounding-error-sized number that a neighbouring cell
dwarfs: `reserve` after run-off is exactly the case, and a per-cell rule would either fail
on 1e-17 ÷ 1e-17 or force an arbitrary "ignore small cells" rule. The scale-relative form
asks the question a reviewer actually asks — *is this component wrong by an amount that
matters against the size of the component?* — and answers it with one number.

`floor` is 1.0 for a component that is solved to zero by construction (`term_solve`'s
`bel`, and the `profit_margin` derived from it): those have no scale of their own, and the
honest comparison is against the solve's own absolute tolerance, `1e-8`.

The threshold is `1e-9`, as the spec requires.
"""

from __future__ import annotations

from dataclasses import dataclass, field

import numpy as np

from .outputs import Outputs

TOLERANCE = 1e-9

#: Components with no scale of their own because they are solved to zero. Compared with a
#: scale floor of 1, i.e. absolutely, against the solve tolerance in `term_solve/run.pir`.
ZERO_TARGETS = {"term_solve": {"bel", "profit_margin"}}


@dataclass
class ComponentResult:
    component: str
    kind: str
    error: float
    cells: int
    passed: bool


@dataclass
class GateResult:
    scenario: str
    size: int
    engine: str
    reference: str
    components: list[ComponentResult] = field(default_factory=list)
    missing: list[str] = field(default_factory=list)
    error: str | None = None

    @property
    def passed(self) -> bool:
        return self.error is None and all(c.passed for c in self.components)

    @property
    def worst(self) -> ComponentResult | None:
        return max(self.components, key=lambda c: c.error, default=None)

    def to_json(self) -> dict:
        worst = self.worst
        return {
            "scenario": self.scenario,
            "size": self.size,
            "engine": self.engine,
            "reference": self.reference,
            "passed": self.passed,
            "components_compared": len(self.components),
            "cells_compared": sum(c.cells for c in self.components),
            "worst_component": worst.component if worst else None,
            "worst_error": worst.error if worst else None,
            "failed": [c.component for c in self.components if not c.passed],
            "not_implemented": self.missing,
            "error": self.error,
        }


def _scale_floor(scenario: str, component: str) -> float:
    return 1.0 if component in ZERO_TARGETS.get(scenario, ()) else 1e-12


def _compare_arrays(scenario, component, kind, a, b) -> ComponentResult:
    a = np.asarray(a, dtype=np.float64)
    b = np.asarray(b, dtype=np.float64)
    if a.shape != b.shape:
        return ComponentResult(component, kind, float("inf"), 0, False)
    scale = max(float(np.max(np.abs(b))) if b.size else 0.0, _scale_floor(scenario, component))
    err = float(np.max(np.abs(a - b))) / scale if a.size else 0.0
    return ComponentResult(component, kind, err, int(a.size), err <= TOLERANCE)


def compare(
    scenario: str, size: int, engine: str, actual: Outputs, reference: Outputs, ref_name: str
) -> GateResult:
    """Compare one engine against the reference on the components both produce."""
    result = GateResult(scenario=scenario, size=size, engine=engine, reference=ref_name)
    order = {k: i for i, k in enumerate(reference.keys)}
    if actual.keys and actual.keys != reference.keys:
        try:
            idx = np.array([order[k] for k in actual.keys])
        except KeyError:
            result.error = "modelpoint keys do not match the reference"
            return result
        inverse = np.argsort(idx)
    else:
        inverse = None

    def align(arr):
        return arr if inverse is None else arr[inverse]

    for kind, mine, theirs in (
        ("per_mp", actual.per_mp, reference.per_mp),
        ("series", actual.series, reference.series),
    ):
        for name, values in sorted(mine.items()):
            if name not in theirs:
                continue
            result.components.append(
                _compare_arrays(scenario, name, kind, align(values), theirs[name])
            )
    result.missing = sorted(reference.component_names() - actual.component_names())
    return result
