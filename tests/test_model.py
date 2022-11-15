from pandas import DataFrame
from src.predictable import Model, StaticFlow


def test_model_components():
    assert isinstance(Model().components, dict)


def test_add_model_component():
    m = Model()
    m.add_component(StaticFlow(label="test", input_array=[1, 2, 3]))
    assert len(m.components) == 1


def test_model_results():
    assert isinstance(Model().results, DataFrame)
