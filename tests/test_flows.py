import pytest
from pandas import DataFrame
from src.predictable import CashFlow, StaticFlow


def test_cashflow_setup():
    c = CashFlow(input_array=[100], label="test")
    assert c.label == "test"
    assert len(c) == 1
    assert c.sum() == 100


def test_cashflow_projection():
    c = CashFlow(input_array=[100], label="test")
    results = c.project(term=10, results=DataFrame())
    assert len(results) == 10 + 1


@pytest.mark.parametrize(
    "input_value, formula, output_value",
    [
        (100, lambda x: x + 100, 200),
        (100, lambda x: x * 1.05, 105),
    ],
)
def test_cashflow_formula_projection(input_value, formula, output_value):
    c = CashFlow(label="test", input_array=[input_value], formula=formula)
    results = c.project(term=1, results=DataFrame())
    assert results[-1] == output_value


def test_staticflow_setup():
    s = StaticFlow(label="test", input_array=[1, 2, 3])
    assert s.label == "test"
    assert len(s) == 3
    assert s.sum() == 6


@pytest.mark.parametrize(
    "projected_period, projected_sum", [(10, 1.5), (1, 1.0), (3, 1.5)]
)
def test_static_discounting_projections(projected_period, projected_sum):
    d = StaticFlow([0.5, 0.5, 0.5], label="test")
    results = d.project(projected_period, results=DataFrame())
    assert results.sum() == projected_sum
