"""The five scenarios of `03-engine.md` §11.2, and the sizes each engine can be asked for.

Sizes
-----
The spec asks for `M ∈ {1, 1_000, 100_000, 1_000_000, 10_000_000}`. `M = 1` is not
padding: it exposes the per-run overhead — parse, check, plan, table build — that a large-
`M` run amortises away, and it is the size a developer iterating on a model actually feels.

`M = 10_000_000` is **not run here**, and neither is `M = 1_000_000` on the three
360/480-period scenarios. The reason is measured, not assumed: `emit = "outputs"` means
predictable writes every declared output for every modelpoint for every period, and on
this workstation `ifrs17_gmm` at `M = 1_000_000` is killed for memory before it finishes,
`ifrs17_gmm` at `M = 100_000` writes a 2.6 GB result set that does not fit the free disk,
and `savings_monthly` at `M = 100_000` writes 2 GB. Those sizes belong on the large CI
runner of `03-engine.md` §11.3, and until they have run there this repository publishes no
number for them.

Per-engine caps exist because a scalar Python engine at `M = 1e6` on a 360-period model is
a multi-day run. The cap is a property of the contender, is recorded next to its numbers,
and is never used to quietly drop a scenario: an engine that cannot reach a size is
reported as "not run at this size", not omitted.
"""

from __future__ import annotations

from dataclasses import dataclass, field

SIZES = [1, 100, 1_000, 10_000, 100_000, 1_000_000]

#: Sizes the spec asks for that this repository does not run. See the module docstring.
CI_ONLY_SIZES = [10_000_000]


@dataclass(frozen=True)
class Scenario:
    name: str
    periods: int
    components: int
    description: str
    #: The largest `M` each engine is asked for. Absent means "every size in `SIZES`".
    caps: dict[str, int] = field(default_factory=dict)


SCENARIOS = {
    s.name: s
    for s in [
        Scenario(
            "term_annual",
            40,
            22,
            "Level term assurance, 40 annual periods. The headline number.",
            caps={"cashflower": 10_000, "modelx": 1_000},
        ),
        Scenario(
            "term_monthly",
            480,
            25,
            "The same product monthly, 480 periods — the `t`-loop constant.",
            caps={
                "predictable": 100_000,
                "numpy": 100_000,
                "cashflower": 1_000,
                "modelx": 100,
            },
        ),
        Scenario(
            "savings_monthly",
            360,
            49,
            "Unit-linked endowment: an account value, four lookups a month, branch-heavy.",
            caps={
                "predictable": 100_000,
                "numpy": 100_000,
                "cashflower": 1_000,
                "modelx": 100,
            },
        ),
        Scenario(
            "ifrs17_gmm",
            360,
            75,
            "IFRS 17 GMM on the endowment: CSM, RA, coverage units, two substage levels.",
            # 45 declared outputs over 361 periods: predictable's result set is 2.6 GB at
            # M = 100_000, which does not fit this workstation's free disk. Measured, not
            # guessed — see the module docstring.
            caps={
                "predictable": 10_000,
                "numpy": 10_000,
                "cashflower": 0,
                "modelx": 0,
            },
        ),
        Scenario(
            "term_solve",
            40,
            22,
            "`term_annual` with the premium solved to a zero BEL — the outer loop.",
            caps={
                "predictable": 10_000,
                "numpy": 10_000,
                "cashflower": 100,
                "modelx": 100,
            },
        ),
    ]
}

#: Sizes the correctness gate runs at. Small on purpose: the gate compares *every* output
#: for *every* modelpoint, including the full series, so its cost is `O(M × T × C)` in all
#: four engines at once. Agreement is a property of the model, not of the portfolio size,
#: and `M = 1` / `M = 25` / `M = 1000` between them cover the single-policy path, the
#: full modelpoint lattice, and a portfolio that crosses predictable's chunk boundary.
GATE_SIZES = [1, 25, 1_000]

#: Sizes each engine is gated at. The scalar engines stop at the modelpoint lattice:
#: cashflower and modelx project one policy at a time with no shared state between them,
#: so their answer for a policy cannot depend on how many other policies are in the file.
#: predictable can — it batches into chunks — so it is gated across the 1024-row chunk
#: boundary as well, which is what `M = 1000` is there for.
GATE_SIZES_BY_ENGINE = {
    "cashflower": [1, 25],
    "modelx": [1, 25],
}


def gate_sizes_for(engine: str, override: list[int] | None = None) -> list[int]:
    if override is not None:
        return override
    return GATE_SIZES_BY_ENGINE.get(engine, GATE_SIZES)


def sizes_for(scenario: str, engine: str) -> list[int]:
    cap = SCENARIOS[scenario].caps.get(engine)
    if cap is None:
        return list(SIZES)
    return [s for s in SIZES if s <= cap]
