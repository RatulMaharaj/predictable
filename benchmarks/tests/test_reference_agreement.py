"""The real cross-engine agreement tests: all four engines, all five scenarios.

These are the benchmark's own regression suite. If a formula in any implementation drifts,
one of these fails long before anyone looks at a timing.

The NumPy reference is additionally pinned to the **committed goldens** of the reference
corpus (`models/<name>/expected/`), which is what makes it a reference rather than a second
opinion: it reproduces the engine's published numbers exactly, bit for bit, and the other
two engines are then compared against it.
"""

from __future__ import annotations

import csv

import pytest

from bench import gate
from bench.basis import MODELS, savings_modelpoints, term_modelpoints
from bench.engines import numpy_ref
from bench.harness import _run_engine, _supported

SCENARIOS = ["term_annual", "term_monthly", "savings_monthly", "ifrs17_gmm", "term_solve"]
CORPUS_SIZE = {
    "term_annual": 25,
    "term_monthly": 25,
    "term_solve": 25,
    "savings_monthly": 20,
    "ifrs17_gmm": 20,
}


def _modelpoints(scenario: str, n: int):
    return term_modelpoints(n) if scenario.startswith("term") else savings_modelpoints(n)


def _golden_per_mp(scenario: str) -> dict[str, dict[str, float]]:
    path = MODELS / scenario / "expected" / "per_mp.csv"
    out: dict[str, dict[str, float]] = {}
    for row in csv.DictReader(path.open()):
        out.setdefault(row["component"].split(".")[-1], {})[row["mp_key"]] = float(row["value"])
    return out


def _golden_series(scenario: str, key: str) -> tuple[list[str], list[dict]]:
    path = MODELS / scenario / "expected" / f"series_{key}.csv"
    rows = list(csv.DictReader(path.open()))
    return list(rows[0].keys()), rows


# -- the NumPy reference against the committed goldens ------------------------------------


@pytest.mark.parametrize("scenario", SCENARIOS)
def test_numpy_reference_reproduces_the_committed_per_mp_goldens(scenario):
    n = CORPUS_SIZE[scenario]
    out = numpy_ref.run(scenario, _modelpoints(scenario, n), collect_series=True)
    index = {k: i for i, k in enumerate(out.keys)}
    checked = 0
    for component, expected in _golden_per_mp(scenario).items():
        if component not in out.per_mp:
            continue
        scale = max(max(abs(v) for v in expected.values()), 1.0)
        for key, value in expected.items():
            assert abs(out.per_mp[component][index[key]] - value) / scale < 1e-9, (
                f"{scenario}.{component} at {key}"
            )
        checked += 1
    assert checked >= 5, f"{scenario}: only {checked} PerMP components were checked"


@pytest.mark.parametrize("scenario", SCENARIOS)
def test_numpy_reference_reproduces_the_committed_series_goldens(scenario):
    n = CORPUS_SIZE[scenario]
    out = numpy_ref.run(scenario, _modelpoints(scenario, n), collect_series=True)
    key = out.keys[0]
    columns, rows = _golden_series(scenario, key)
    checked = 0
    for column in columns:
        if column == "t":
            continue
        component = column.split(".")[-1]
        if component not in out.series:
            continue
        expected = [float(r[column]) for r in rows]
        scale = max(max(abs(v) for v in expected), 1.0)
        mine = out.series[component][out.keys.index(key)]
        for t, value in enumerate(expected):
            assert abs(mine[t] - value) / scale < 1e-9, f"{scenario}.{component} at t={t}"
        checked += 1
    assert checked >= 5, f"{scenario}: only {checked} series components were checked"


# -- every engine against the NumPy reference ---------------------------------------------


@pytest.mark.parametrize("scenario", SCENARIOS)
@pytest.mark.parametrize("engine", ["predictable", "cashflower", "modelx"])
def test_engines_agree_with_the_reference_on_the_modelpoint_lattice(engine, scenario):
    if not _supported(engine, scenario):
        pytest.skip(f"{engine} does not implement {scenario}")
    size = CORPUS_SIZE[scenario]
    reference = _run_engine("numpy", scenario, size, collect_series=True)
    actual = _run_engine(engine, scenario, size, collect_series=True)
    result = gate.compare(scenario, size, engine, actual, reference, "numpy")
    assert result.components, "nothing was compared — the engines share no component names"
    assert result.passed, [
        (c.component, c.error) for c in result.components if not c.passed
    ]


@pytest.mark.parametrize("scenario", ["term_annual", "savings_monthly"])
def test_a_single_modelpoint_agrees_too(scenario):
    """`M = 1` takes a different path in predictable — one chunk, one lane."""
    reference = _run_engine("numpy", scenario, 1, collect_series=True)
    for engine in ("predictable", "cashflower", "modelx"):
        actual = _run_engine(engine, scenario, 1, collect_series=True)
        assert gate.compare(scenario, 1, engine, actual, reference, "numpy").passed, engine


def test_predictable_agrees_across_a_chunk_boundary():
    """1000 modelpoints spans predictable's 1024-row chunk; 2000 crosses it."""
    for size in (1000, 2000):
        reference = _run_engine("numpy", "term_annual", size, collect_series=True)
        actual = _run_engine("predictable", "term_annual", size, collect_series=True)
        assert gate.compare("term_annual", size, "predictable", actual, reference, "numpy").passed
