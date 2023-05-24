model_code = """import predictable as pr


def handler(modelpoint, **config) -> dict:
    # NOTE: **config used to access info from RunConfig e.g.
    # NOTE: `config[table_location]` can be used in TableLookup components
    # NOTE: `config[results_location]` can be used to store results on a modelpoint level

    # Instantiate a a model
    model = pr.Model()

    # Project over a given term
    df: pr.DataFrame = model.project(term=120 - modelpoint.age)

    return df.to_dict()
"""

run_code = """from predictable import RunConfig, dataclass
from model import handler


# Define what a modelpoint looks like
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
base_run = RunConfig(
    handler=handler,
    modelpoint_definition=ModelPoint,
    modelpoint_file="data/modelpoints.csv",
    table_location="tables",
    results_location="results",
    label="base_run",
)

base_run.run()
"""

modelpoints_csv = """policy_number,age,gender,smoker_status,premium,cover,expenses
001,23,M,NS,100,1000000,10"""
