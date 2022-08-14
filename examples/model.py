# `pip install -e .` to install the library
# import the library
from divination import (
    CashFlow,
    DiscountFactors,
    Model,
    StaticFlow,
    set_precision,
)

# Create new model instance
model = Model()

# Set rounding level
# returns precision but can also call get_precision
rounding = set_precision(4)

# Add a premium component
# The 'formula' can be used to adjust the values of the cashflows over time
prem = CashFlow(
    input_array=[100],
    formula=lambda prev: prev * 1.05,
    label="premium",
)
# Register the component
model.add_component(prem)

# Add a cover component
cover = CashFlow(
    label="cover",
    input_array=[1_000_000],
)
# Register the component
model.add_component(cover)

# Add an expense component
exp = StaticFlow(
    label="expense",
    input_array=[10 for _ in range(0, 5)],
)
# Register the component
model.add_component(exp)

# Add discounting component
disc = DiscountFactors(interest_rate=0.05, label="V")
# Register the component
model.add_component(disc)

# TODO: define relationships between variables somehow?
# Allow the creation of derived cashflow vectors using labels
# (premium - cover - expense ) * discounting

# Add derived component - registers automatically
# model.derive_component(label="reserve", expression="premium - cover")

# Project cashflows over term
results = model.project(term=10)

# Results get returned as a pandas dataframe
print(results)

# TODO: How this 'model.py' will be applied to modelpoints is still TBD
# TODO: Inputs to many of the cashflow vectors would need to come from the data
# TODO: This example can be thought off as applying to a single policy
