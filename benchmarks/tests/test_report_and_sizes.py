"""The reporting layer, and the size caps it reports on."""

from __future__ import annotations

import json

import pytest

from bench import docs_update, gate, report as report_mod, scenarios
from bench.harness import Report
from bench.timing import Timing


def _timing(scenario="term_annual", size=1000, engine="predictable", seconds=(0.1,), threads=1):
    return Timing(
        scenario=scenario,
        size=size,
        engine=engine,
        threads=threads,
        periods=40,
        cold_s=seconds[0] * 2,
        runs_s=list(seconds),
        peak_rss_mb=12.0,
    )


def _passing_gate(scenario="term_annual", engine="numpy"):
    r = gate.GateResult(scenario, 25, engine, "numpy")
    r.components.append(gate.ComponentResult("bel", "per_mp", 0.0, 25, True))
    return r


# -- sizes ---------------------------------------------------------------------------------


def test_sizes_are_capped_per_engine_and_never_silently_dropped():
    full = scenarios.sizes_for("term_annual", "predictable")
    capped = scenarios.sizes_for("term_annual", "modelx")
    assert full == scenarios.SIZES
    assert capped and max(capped) <= scenarios.SCENARIOS["term_annual"].caps["modelx"]
    assert set(capped) < set(full)


def test_an_engine_with_a_zero_cap_gets_no_sizes():
    assert scenarios.sizes_for("ifrs17_gmm", "cashflower") == []


def test_the_ci_only_size_is_declared_and_not_in_the_run_set():
    assert 10_000_000 in scenarios.CI_ONLY_SIZES
    assert 10_000_000 not in scenarios.SIZES


# -- the timing record ----------------------------------------------------------------------


def test_median_is_used_not_mean_so_one_outlier_does_not_move_it():
    t = _timing(seconds=(0.10, 0.10, 0.10, 0.10, 5.0))
    assert t.median_s == 0.10
    assert t.to_json()["max_s"] == 5.0


def test_mp_periods_per_second_counts_the_zero_period():
    t = _timing(size=1000, seconds=(1.0,))
    assert t.mp_periods_per_s == 1000 * 41


# -- the markdown report ---------------------------------------------------------------------


def test_report_marks_a_failed_gate_and_names_the_scenario():
    failing = gate.GateResult("term_annual", 25, "cashflower", "numpy")
    failing.components.append(gate.ComponentResult("bel", "per_mp", 1e-3, 25, False))
    report = Report(environment={"platform": "test"}, gate=[failing])
    report.skipped = ["term_annual: not timed — the correctness gate failed"]
    text = report_mod.render(report)
    assert "**FAIL**" in text
    assert "not timed" in text
    assert not report.gate_passed


def test_report_computes_speedups_against_predictable():
    report = Report(
        environment={"platform": "test"},
        gate=[_passing_gate()],
        timings=[
            _timing(engine="predictable", seconds=(0.1,)),
            _timing(engine="cashflower", seconds=(2.5,)),
        ],
    )
    text = report_mod.render(report)
    assert "25.0×" in text


def test_a_failed_measurement_keeps_its_row_and_states_why():
    """A configuration that raised must be visible, not silently absent from the table."""
    failed = Timing(
        scenario="savings_monthly",
        size=100_000,
        engine="predictable",
        threads=1,
        periods=360,
        cold_s=float("nan"),
        runs_s=[],
        note="failed: RuntimeError: predictable run failed (2)",
    )
    report = Report(
        environment={},
        gate=[_passing_gate(scenario="savings_monthly")],
        timings=[failed, _timing(engine="cashflower", seconds=(1.0,))],
    )
    assert not failed.ok
    text = report_mod.render(report)
    assert "failed: RuntimeError" in text
    assert "nan" not in text
    # `NaN` is not valid JSON; a failed row serialises as nulls.
    payload = json.loads(json.dumps(failed.to_json()))
    assert payload["median_s"] is None
    assert payload["ok"] is False


def test_a_failed_measurement_is_excluded_from_speedups_and_targets(tmp_path):
    failed = Timing(
        scenario="term_annual",
        size=1000,
        engine="predictable",
        threads=1,
        periods=40,
        cold_s=float("nan"),
        runs_s=[],
        note="failed: boom",
    )
    report = Report(
        environment={},
        gate=[_passing_gate()],
        timings=[failed, _timing(engine="cashflower", seconds=(1.0,))],
    )
    assert report_mod._speedups(report) == {}
    page = tmp_path / "benchmarks.md"
    page.write_text("<!-- GATE-SUMMARY -->\n<!-- THROUGHPUT -->\n<!-- TARGETS -->\n")
    docs_update.update(report, page)
    assert "nan" not in page.read_text()


def test_report_states_the_size_it_did_not_run():
    text = report_mod.render(Report(environment={}, gate=[_passing_gate()]))
    assert "10,000,000" in text
    assert "not implemented in cashflower, modelx" in text


# -- the docs page -----------------------------------------------------------------------------


def test_docs_blocks_are_replaced_in_place_and_are_idempotent(tmp_path):
    page = tmp_path / "benchmarks.md"
    page.write_text(
        "intro\n\n<!-- GATE-SUMMARY -->\n\n<!-- THROUGHPUT -->\n\n<!-- TARGETS -->\n\nouttro\n"
    )
    report = Report(
        environment={},
        gate=[_passing_gate()],
        timings=[_timing(engine="predictable", size=1, seconds=(0.01,))],
    )
    docs_update.update(report, page)
    first = page.read_text()
    docs_update.update(report, page)
    assert page.read_text() == first
    assert "intro" in first and "outtro" in first
    assert "**1 comparisons**, 25 cells" in first
    assert "**0 failed.**" in first


def test_docs_target_table_says_not_run_rather_than_comparing_a_smaller_size(tmp_path):
    page = tmp_path / "benchmarks.md"
    page.write_text("<!-- GATE-SUMMARY -->\n<!-- THROUGHPUT -->\n<!-- TARGETS -->\n")
    report = Report(
        environment={},
        gate=[_passing_gate()],
        timings=[_timing(scenario="term_annual", size=100_000, seconds=(1.0,))],
    )
    docs_update.update(report, page)
    text = page.read_text()
    assert "not run at this size" in text
    assert "largest run: 100,000" in text


def test_a_missing_marker_is_an_error_not_a_silent_no_op(tmp_path):
    page = tmp_path / "benchmarks.md"
    page.write_text("no markers here\n")
    with pytest.raises(KeyError):
        docs_update.update(Report(environment={}), page)
