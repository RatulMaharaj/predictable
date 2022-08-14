# import the library
import divination as dv

# Create new model instance
model = dv.Model()

# Set rounding level
# returns precision but can also call get_precision
rounding = dv.set_precision(4)

# Add a premium component
# The 'formula' can be used to adjust the values of the cashflows over time
prem = dv.CashFlow(
    label="premium",
    formula=lambda prev: prev * 1.05,
    initial_data=[100],
)
model.add_component(prem)

# Add a cover component
cover = dv.CashFlow(
    label="cover",
    initial_data=[1_000_000],
)
model.add_component(cover)

# Add an expense component
exp = dv.StaticFlow(
    label="expense",
    data=[10 for _ in range(0, 5)],
)
model.add_component(exp)

# Add discounting
disc = dv.DiscountFactors(interest_rate=0.05, label="discounting")
model.add_component(disc)

# TODO: define relationships between variables somehow?
# Allow the creation of derived cashflow vectors using labels
# (premium - cover - expense ) * discounting

# Project cashflows over term
results = model.project(term=10)

# Results get returned as a pandas dataframe
print(results)

# TODO: How this 'model.py' will be applied to modelpoints is still TBD
# TODO: Inputs to many of the cashflow vectors would need to come from the data
# TODO: This example can be thought off as applying to a single policy
