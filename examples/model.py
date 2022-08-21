# `pip install -e .` to install the library in development
# import the library

import divinate as dv


def handler(modelpoint, **kwargs):
    # Create new model instance
    model = dv.Model()

    # Add static components
    age = dv.RatingFactor(
        input_array=[modelpoint.age],
        formula=lambda prev: prev + 1,
        label="age",
    )
    gender = dv.RatingFactor(input_array=[modelpoint.gender], label="gender")
    smoker = dv.RatingFactor(
        input_array=[modelpoint.smoker_status], label="smoker"
    )

    # Add components that have a projected value
    prem = dv.CashFlow(
        input_array=[modelpoint.premium],
        formula=lambda prev: prev * 1.05,
        label="premium",
    )
    cover = dv.CashFlow(input_array=[modelpoint.cover], label="cover")
    exp = dv.StaticFlow(
        input_array=[modelpoint.expenses for _ in range(5)],
        label="expense",
    )

    # Add actuarial components
    qx = dv.TableLookup(
        table_name=kwargs["table_location"] / "sa8990.csv",
        lookup_on=["age"],
        keep_column="qx",
        label="qx",
    )
    lapses = dv.TableLookup(
        table_name=kwargs["table_location"] / "lapses.csv",
        lookup_on=["t"],
        keep_column="lapse_rate_pa",
        default_value=0,
        label="lapse",
    )

    v = dv.DiscountFactors(interest_rate=0.05, label="V")

    # Attach components to the model
    component_list = [age, gender, smoker, prem, cover, exp, v, qx, lapses]
    model.add_components(component_list)

    # Project over a given term
    # Results return a pandas df object
    df = model.project(term=33 - modelpoint.age)

    # Perform linear combination style manipulations
    # Discounting the components
    components = ["premium", "cover", "expense"]
    for component in components:
        df[f"EV_{component}"] = (
            df[component] * df["V"] * df["qx"] * df["lapse"]
        )

    # Define reserving relationship
    df["Reserve"] = df["EV_cover"] + df["EV_expense"] - df["EV_premium"]

    # Results get returned as a pandas dataframe

    # epv = df[["EV_premium", "EV_cover", "EV_expense", "Reserve"]].sum()
    # print(epv)
    return df


if __name__ == "__main__":
    # Define input data format
    @dv.dataclass
    class ModelPoint:
        policy_number: str
        age: int
        gender: str
        smoker_status: str
        premium: float
        cover: float
        expenses: float

    # Test modelpoint
    modelpoint = ModelPoint(
        policy_number="001",
        age=23,
        gender="M",
        smoker_status="NS",
        premium=100.00,
        cover=1_000_000.00,
        expenses=10,
    )

    print(handler(modelpoint))
