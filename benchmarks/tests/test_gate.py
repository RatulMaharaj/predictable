"""The gate has to be able to fail. These tests prove it can."""

from __future__ import annotations

import numpy as np
import pytest

from bench import gate
from bench.outputs import Outputs


def _outputs(keys, per_mp=None, series=None):
    return Outputs(keys=list(keys), per_mp=per_mp or {}, series=series or {})


def test_identical_outputs_pass_with_zero_error():
    ref = _outputs(["A", "B"], per_mp={"bel": np.array([100.0, -50.0])})
    result = gate.compare("term_annual", 2, "numpy", ref, ref, "numpy")
    assert result.passed
    assert result.worst.error == 0.0
    assert result.worst.cells == 2


def test_a_difference_above_tolerance_fails():
    ref = _outputs(["A"], per_mp={"bel": np.array([1000.0])})
    actual = _outputs(["A"], per_mp={"bel": np.array([1000.0 * (1 + 1e-8)])})
    result = gate.compare("term_annual", 1, "other", actual, ref, "numpy")
    assert not result.passed
    assert result.worst.error == pytest.approx(1e-8, rel=1e-3)


def test_a_difference_below_tolerance_passes():
    ref = _outputs(["A"], per_mp={"bel": np.array([1000.0])})
    actual = _outputs(["A"], per_mp={"bel": np.array([1000.0 * (1 + 1e-12)])})
    assert gate.compare("term_annual", 1, "other", actual, ref, "numpy").passed


def test_error_is_relative_to_the_components_own_scale_not_the_cell():
    """A cell that is rounding noise next to its neighbours must not dominate the metric.

    `reserve` after run-off is exactly this: 1e-13 against a component whose scale is
    thousands. Cell-relative would report 100% error; scale-relative reports 1e-16.
    """
    ref = _outputs(["A"], series={"reserve": np.array([[5000.0, 1e-13]])})
    actual = _outputs(["A"], series={"reserve": np.array([[5000.0, 2e-13]])})
    result = gate.compare("term_annual", 1, "other", actual, ref, "numpy")
    assert result.passed
    assert result.worst.error == pytest.approx(1e-13 / 5000.0)


def test_a_component_solved_to_zero_is_compared_absolutely():
    """`term_solve`'s `bel` has no scale of its own; 3e-10 must pass, 3e-8 must not."""
    ref = _outputs(["A"], per_mp={"bel": np.array([1e-12])})
    ok = _outputs(["A"], per_mp={"bel": np.array([1e-12 + 3e-10])})
    bad = _outputs(["A"], per_mp={"bel": np.array([1e-12 + 3e-8])})
    assert gate.compare("term_solve", 1, "other", ok, ref, "numpy").passed
    assert not gate.compare("term_solve", 1, "other", bad, ref, "numpy").passed
    # The same numbers in a scenario with no zero target are judged on the real scale.
    assert not gate.compare("term_annual", 1, "other", ok, ref, "numpy").passed


def test_reordered_modelpoints_are_aligned_before_comparison():
    ref = _outputs(["A", "B", "C"], per_mp={"bel": np.array([1.0, 2.0, 3.0])})
    shuffled = _outputs(["C", "A", "B"], per_mp={"bel": np.array([3.0, 1.0, 2.0])})
    assert gate.compare("term_annual", 3, "other", shuffled, ref, "numpy").passed


def test_unknown_modelpoint_keys_are_an_error_not_a_pass():
    ref = _outputs(["A"], per_mp={"bel": np.array([1.0])})
    other = _outputs(["Z"], per_mp={"bel": np.array([1.0])})
    result = gate.compare("term_annual", 1, "other", other, ref, "numpy")
    assert not result.passed
    assert "keys do not match" in result.error


def test_components_the_engine_does_not_produce_are_reported_as_missing():
    ref = _outputs(["A"], per_mp={"bel": np.array([1.0]), "csm_initial": np.array([2.0])})
    partial = _outputs(["A"], per_mp={"bel": np.array([1.0])})
    result = gate.compare("ifrs17_gmm", 1, "other", partial, ref, "numpy")
    assert result.passed
    assert result.missing == ["csm_initial"]


def test_a_shape_mismatch_fails_rather_than_broadcasting():
    ref = _outputs(["A"], series={"deaths": np.zeros((1, 41))})
    short = _outputs(["A"], series={"deaths": np.zeros((1, 40))})
    assert not gate.compare("term_annual", 1, "other", short, ref, "numpy").passed
