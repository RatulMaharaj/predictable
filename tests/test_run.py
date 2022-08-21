import pytest
from src.divinate import RunConfig, dataclass


def test_create_run_settings():
    @dataclass
    class ModelPoint:
        test: str

    def handler(modelpoint):
        pass

    with pytest.raises(FileNotFoundError):
        RunConfig(
            handler,
            ModelPoint,
            modelpoint_file="/path/to/file/that/does/not/exist.csv",
            results_location=None,
        )
