import pytest
from src.predictable import PRECISION, get_precision, set_precision


def test_get_precision():
    assert get_precision() == PRECISION


@pytest.mark.parametrize("input_value, expected", [(2, 2), (2.0, 2), ("2", 2)])
def test_set_precision_type(input_value, expected):
    set_precision(input_value)
    assert get_precision() == expected


@pytest.mark.parametrize("input_value", [-2, -2.0, "-2"])
def test_set_negative_precision(input_value):
    with pytest.raises(ValueError):
        set_precision(input_value)
