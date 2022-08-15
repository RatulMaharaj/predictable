# `pip install -e .` to install the library
# import the library
from divinate import CashFlow, DiscountFactors, Model, StaticFlow

# Create new model instance
model = Model()

# Add a premium component
model.add_component(
    CashFlow(
        input_array=[100], formula=lambda prev: prev * 1.05, label="premium"
    )
)

# Add a cover component
model.add_component(CashFlow(label="cover", input_array=[1_000_000]))

# Add an expense component
model.add_component(
    StaticFlow(
        input_array=[10, 10, 10, 10, 10],
        label="expense",
    )
)

# Add discounting component
model.add_component(DiscountFactors(interest_rate=0.05, label="V"))

# Project cashflows over term
# Results return a pandas df object
df = model.project(term=10)

# Perform linear combination style manipulations
# Discounting the components
components = ["premium", "cover", "expense"]
for component in components:
    df[f"V_{component}"] = df[component] * df["V"]


# Define reserving relationship
df["Reserve"] = df["V_cover"] + df["V_expense"] - df["V_premium"]

# Results get returned as a pandas dataframe
print(df)

# TODO: How this 'model.py' will be applied to modelpoints is still TBD
# TODO: Inputs to many of the cashflow vectors would need to come from the data
# TODO: This example can be thought off as applying to a single policy
