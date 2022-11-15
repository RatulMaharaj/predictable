import pytest
from pandas import DataFrame
from src.predictable import DiscountFactors, i_to_v


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
    results = d.project(1, results=DataFrame())
    assert [round(i, 6) for i in results] == [1, 0.952381]
    assert len(results) == 2
