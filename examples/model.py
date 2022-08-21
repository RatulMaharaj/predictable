# `pip install -e .` to install the library in development
# import the library
import divinate as dv


def handler(modelpoint):
    # Create new model instance
    model = dv.Model()

    # Add basic rating factor components
    model.add_component(
        dv.RatingFactor(
            input_array=[modelpoint.age],
            formula=lambda prev: prev + 1,
            label="age",
        )
    )
    model.add_component(
        dv.RatingFactor(input_array=[modelpoint.gender], label="gender")
    )

    model.add_component(
        dv.RatingFactor(input_array=[modelpoint.smoker_status], label="smoker")
    )

    # Add a premium component
    model.add_component(
        dv.CashFlow(
            input_array=[modelpoint.premium],
            formula=lambda prev: prev * 1.05,
            label="premium",
        )
    )

    # Add a cover component
    model.add_component(
        dv.CashFlow(input_array=[modelpoint.cover], label="cover")
    )

    # Add an expense component
    model.add_component(
        dv.StaticFlow(
            input_array=[modelpoint.expenses for _ in range(5)],
            label="expense",
        )
    )

    # Add risk rate
    # TODO: Allow this to be based on a table and a combination of factors
    model.add_component(dv.RiskRates(input_array=[0.001], label="qx"))

    # Add discounting component
    model.add_component(dv.DiscountFactors(interest_rate=0.05, label="V"))

    # Project dv.cashflows over term
    # Results return a pandas df object
    df = model.project(term=10)

    # Perform linear combination style manipulations
    # Discounting the components
    components = ["premium", "cover", "expense"]
    for component in components:
        df[f"EV_{component}"] = df[component] * df["V"] * df["qx"]

    # Define reserving relationship
    df["Reserve"] = df["EV_cover"] + df["EV_expense"] - df["EV_premium"]

    # Results get returned as a pandas dataframe
    return df

    # TODO: How this 'model.py' will be applied to modelpoints is still TBD
    # TODO: Inputs to many of the dv.cashflow vectors would need to come from the data
    # TODO: This example can be thought off as applying to a single policy


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

    handler(modelpoint)
