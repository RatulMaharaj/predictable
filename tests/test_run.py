import pytest
from src.predictable import RunConfig, dataclass


@dataclass
class ModelPoint:
    test: str


def handler(modelpoint):
    pass


def test_create_instance():
    rc = RunConfig(
        handler=handler,
        modelpoint_definition=ModelPoint,
        table_location=None,
        modelpoint_file=None,
        results_location=None,
        label="test_config",
    )
    assert isinstance(rc, RunConfig)


def test_create_multiple_instances():
    rc2 = RunConfig(
        handler=handler,
        modelpoint_definition=ModelPoint,
        table_location=None,
        modelpoint_file=None,
        results_location=None,
        label="test_config_2",
    )
    assert isinstance(rc2, RunConfig)
    # test that instances get added to the instances list
    assert len(RunConfig.instances) == 2


def test_missing_modelpoint_file():
    with pytest.raises(FileNotFoundError):
        RunConfig(
            handler,
            ModelPoint,
            modelpoint_file="/path/to/file/that/does/not/exist.csv",
            results_location=None,
        )


def test_missing_table_location():
    with pytest.raises(FileNotFoundError):
        RunConfig(
            handler=handler,
            modelpoint_definition=ModelPoint,
            table_location="/path/to/fictional/table/location",
            modelpoint_file=None,
            results_location=None,
        )
