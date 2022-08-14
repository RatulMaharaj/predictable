import pytest
from divination.core.flows import CashFlow, StaticFlow


def test_cashflow_setup():
    c = CashFlow(label="test", initial_data=[100])

    assert c.label == "test"
    assert c.data == [100]
    assert c.length == 1


def test_cashflow_projection():
    c = CashFlow(label="test", initial_data=[100])
    c.project(term=10)
    assert c.length == 10 + 1


@pytest.mark.parametrize(
    "input_value, formula, output_value",
    [
        (100, lambda x: x + 100, 200),
        (100, lambda x: x * 1.05, 105),
    ],
)
def test_cashflow_formula_projection(input_value, formula, output_value):
    c = CashFlow(label="test", initial_data=[input_value], formula=formula)
    c.project(term=1)
    assert c.data[-1] == output_value


def test_staticflow_setup():
    s = StaticFlow(label="test", data=[1, 2, 3])
    assert s.label == "test"
    assert s.data == [1, 2, 3]
    assert s.length == 3
