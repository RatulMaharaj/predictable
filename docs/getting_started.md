---
hide-toc: true
---

# Getting started

## Installation

To use Predictable, install it using pip:

```sh
pip install predictable
```

## Quick start example

Create a `model.py` file which will be used to house the modelling logic which will be applied to each modelpoint.

Import in the library as follows:

```python
import predictable as dv
```

We must first create a new instance of the `predictable.core.Model` object.

```python
model = dv.Model()
```

One of the basic building blocks provided in the `core` library is the `CashFlow` component. We can create one as follows:

```python
prem = dv.CashFlow(input_array=[100], formula=lambda prev: prev * 1.05, label="premium")
```

Once we have instantiated a new model, we can associate various cashflow (or similar) component with the model. We can do this for the `prem` component created above as follows:

```python
model.add_component(prem)
```

We'll add some additional components for cover, expenses and discounting:

```python
# Add a sum assured component
cover = dv.CashFlow(label="cover", input_array=[1_000_000])
model.add_component(cover)

# Add an expense component
expenses = dv.StaticFlow(input_array=[10, 10, 10, 10, 10], label="expenses")
model.add_component(expenses)

# Add discounting component
disc = dv.DiscountFactors(interest_rate=0.05, label="V")
model.add_component(disc)
```

Once we are happy with the structure of the model we can define what the _model run_ behaviour looks like.

We would first want to call the project method on the model. This in turn calls the project method on each component of the model to populate a `pandas.DataFrame` with the projected values of each component.

```python
# Project cashflows over a term
df = model.project(term=10)
```

We can easily perform further manipulations on the returned projections. For example, we might want to create new columns which are linear combinations of the existing ones:

```python
# Discounting each of the components
components = ["premium", "cover", "expense"]
for component in components:
    df[f"V_{component}"] = df[component] * df["V"]

# Create a column with the reserve value
df["Reserve"] = df["V_cover"] + df["V_expense"] - df["V_premium"]
```

Results get returned as a pandas dataframe and can therefore be manipulated as such.

```python
print(df.head())
```
