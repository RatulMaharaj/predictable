from divinate import RunSetting, dataclass

from model import handler


# This is just the pydantic dataclass
@dataclass
class ModelPoint:
    policy_number: str
    age: int
    gender: str
    smoker_status: str
    premium: float
    cover: float
    expenses: float


# Define run settings
base_run = RunSetting(
    handler=handler,
    modelpoint_definition=ModelPoint,
    modelpoint_file="examples/modelpoints.csv",
    results_location="examples/output",
)

# run the process
base_run.run()
