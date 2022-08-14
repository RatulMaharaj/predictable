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

# define relationships between variables somehow?

# Project cashflows over term
results = model.project(term=10)

# Results get returned as a pandas dataframe
print(results)
