import pytest
from src.divinate import RunSetting, dataclass


def test_create_run_settings():
    @dataclass
    class ModelPoint:
        test: str

    def handler(modelpoint):
        pass

    with pytest.raises(FileNotFoundError):
        RunSetting(
            handler,
            ModelPoint,
            modelpoint_file="/path/to/file/that/does/not/exist.csv",
            results_location=None,
        )
