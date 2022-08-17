# `pip install -e .` to install the library
# import the library
from dataclasses import dataclass

from divinate import (
    CashFlow,
    DiscountFactors,
    Model,
    RatingFactor,
    RiskRates,
    StaticFlow,
)

# Create new model instance
model = Model()


# Define input data format
@dataclass
class ModelPoint:
    policy_number: str
    age: int
    gender: str
    smoker_status: str
    premium: float
    cover: float
    expenses: float


# Test modelpoint
mpt = ModelPoint(
    policy_number="001",
    age=23,
    gender="M",
    smoker_status="NS",
    premium=100.00,
    cover=1_000_000.00,
    expenses=10,
)

print(mpt)

# Add basic rating factor components
model.add_component(
    RatingFactor(
        input_array=[mpt.age],
        formula=lambda prev: prev + 1,
        label="age",
    )
)
model.add_component(RatingFactor(input_array=[mpt.gender], label="gender"))

model.add_component(
    RatingFactor(input_array=[mpt.smoker_status], label="smoker")
)

# Add a premium component
model.add_component(
    CashFlow(
        input_array=[mpt.premium],
        formula=lambda prev: prev * 1.05,
        label="premium",
    )
)

# Add a cover component
model.add_component(CashFlow(input_array=[mpt.cover], label="cover"))

# Add an expense component
model.add_component(
    StaticFlow(
        input_array=[mpt.expenses for _ in range(5)],
        label="expense",
    )
)

# Add risk rate
# TODO: Allow this to be based on a table and a combination of factors
model.add_component(RiskRates(input_array=[0.001], label="qx"))

# Add discounting component
model.add_component(DiscountFactors(interest_rate=0.05, label="V"))

# Project cashflows over term
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
print(df)

# TODO: How this 'model.py' will be applied to modelpoints is still TBD
# TODO: Inputs to many of the cashflow vectors would need to come from the data
# TODO: This example can be thought off as applying to a single policy
