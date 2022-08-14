import pytest
from divination.core.discounting import (
    DiscountFactors,
    StaticDiscountFactors,
    i_to_v,
)


@pytest.mark.parametrize(
    "i, v", [(0.05, 0.952381), (1, 0.5), (1.25, 0.444444)]
)
def test_i_to_v(i, v):
    assert round(i_to_v(i), 6) == v


def test_discounting_setup():
    d = DiscountFactors(interest_rate=0.05, label="test")

    assert d.label == "test"
    assert d.data == [1]
    assert d.length == 1
    assert d.interest_rate == 0.05


def test_discounting_projection():
    d = DiscountFactors(interest_rate=0.05, label="test")
    results = d.project(1)
    assert [round(i, 6) for i in results] == [1, 0.952381]
    assert d.length == 2


def test_static_discounting_setup():
    d = StaticDiscountFactors([1, 0.95, 0.5], label="test")

    assert d.label == "test"
    assert d.data == [1, 0.95, 0.5]
    assert d.length == 3
