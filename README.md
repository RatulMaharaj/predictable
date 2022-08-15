# Divinate

[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](https://opensource.org/licenses/MIT)
[![pytest](https://github.com/RatulMaharaj/divinate/actions/workflows/pytest.yaml/badge.svg?branch=main)](https://github.com/RatulMaharaj/divinate/actions/workflows/pytest.yaml)

## What is it?

A framework for Actuarial modelling.

## Installation

```sh
pip install divinate # Not yet available
```

## Quick start example

A `model.py` file will be used to house the modelling logic which will be applied to each modelpoint.

```python
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

# Add a sum assured component
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
```

## License

[MIT](https://github.com/RatulMaharaj/divinate/blob/main/LICENSE)

## Documentation

This project is documented using sphinx and the full documentation can be found at **TBD**.

## Development & Contibutions

The following steps can be followed to set up a development environment.

1. Clone the project:

```sh
git clone https://github.com/RatulMaharaj/divinate.git
cd divinate
```

2. Create a virtual environment and activate it using:

```sh
python -m venv venv
source venv/bin/activate # mac
venv\Scripts\activate # windows
```

3. Install the project dependencies:

```sh
pip install -r requirements-dev.txt
```

In development mode, the package can be installed by running:

```sh
pip install -e .
```

4. Install the pre-commit hooks

```sh
pre-commit install
```

### Testing

The tests for this project can be found in the `divinate/tests` directory. Tests will run after every commit (locally) and on every push (using github actions) but can also be run manually using:

```sh
pytest
```
