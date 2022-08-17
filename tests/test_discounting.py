import pytest
from src.divinate import DiscountFactors, StaticDiscountFactors, i_to_v


@pytest.mark.parametrize(
    "i, v", [(0.05, 0.952381), (1, 0.5), (1.25, 0.444444)]
)
def test_i_to_v(i, v):
    assert round(i_to_v(i), 6) == v


def test_discounting_setup():
    d = DiscountFactors(interest_rate=0.05, label="test")

    assert d.label == "test"
    assert len(d) == 1
    assert d.interest_rate == 0.05


def test_discounting_projection():
    d = DiscountFactors(interest_rate=0.05, label="test")
    assert len(d) == 1
    results = d.project(1)
    assert [round(i, 6) for i in results] == [1, 0.952381]
    assert len(results) == 2


def test_static_discounting_setup():
    d = StaticDiscountFactors(input_array=[1, 0.95, 0.5], label="test")
    assert d.label == "test"
    assert len(d) == 3
    assert d.sum() == 1 + 0.95 + 0.5


@pytest.mark.parametrize(
    "projected_period, projected_sum", [(10, 1.5), (1, 1.0), (3, 1.5)]
)
def test_static_discounting_projections(projected_period, projected_sum):
    d = StaticDiscountFactors([0.5, 0.5, 0.5], label="test")
    results = d.project(projected_period)
    assert results.sum() == projected_sum
