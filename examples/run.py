from predictable import RunConfig, dataclass

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


# TODO: Think about whether we want external config files e.g. *_run.toml
# TODO: Think about how multiple files would work in a single directory e.g. *.csv
# TODO: Think about implementing different input formats

# Define run settings
base_run = RunConfig(
    handler=handler,
    modelpoint_definition=ModelPoint,
    modelpoint_file="examples/input/modelpoints.csv",
    table_location="examples/tables",
    results_location="examples/output",
    label="base_run",
)

# Can create multiple run settings instances in one file
cls_run = RunConfig(
    handler=handler,
    modelpoint_definition=ModelPoint,
    modelpoint_file="examples/input/modelpoints.csv",
    table_location="examples/tables",
    results_location="examples/output",
    label="cls_run",
)

# execute a specific run
base_run.run()

# Uncomment line below to easily run all `RunConfig` definitions
# RunConfig.run_all()
