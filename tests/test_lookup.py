import pytest
from src.predictable import TableLookup


def test_tablelookup():
    # Create a new instance
    tbl = TableLookup(
        table_name="test.csv",
        lookup_on="age",
        default_value=0,
        label="test",
        keep_column="test",
    )
    assert isinstance(tbl, TableLookup)


def test_missing_lookup_on():
    with pytest.raises(ValueError):
        TableLookup(
            table_name="test.csv",
            default_value=0,
            label="test",
            keep_column="test",
        )


def test_missing_keep_col():
    with pytest.raises(ValueError):
        TableLookup(
            table_name="test.csv",
            default_value=0,
            label="test",
            lookup_on=["test"],
        )


@pytest.mark.parametrize("default_value", [0, 5.0, "Test"])
def test_defaults(default_value):
    tbl = TableLookup(
        table_name="test.csv",
        default_value=default_value,
        label="test",
        lookup_on=["test"],
        keep_column="test",
    )
    assert tbl.default_value == default_value
