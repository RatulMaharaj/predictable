# Predictable

[![Hatch project](https://img.shields.io/badge/%F0%9F%A5%9A-Hatch-4051b5.svg)](https://github.com/pypa/hatch)
[![Ruff](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/charliermarsh/ruff/main/assets/badge/v1.json)](https://github.com/charliermarsh/ruff)
![PyPI](https://img.shields.io/pypi/v/predictable)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](https://opensource.org/licenses/MIT)
[![pytest](https://github.com/RatulMaharaj/predictable/actions/workflows/pytest.yaml/badge.svg?branch=main)](https://github.com/RatulMaharaj/predictable/actions/workflows/pytest.yaml)
[![build](https://github.com/RatulMaharaj/predictable/actions/workflows/build.yaml/badge.svg?branch=main)](https://github.com/RatulMaharaj/predictable/actions/workflows/build.yaml)
[![Documentation Status](https://readthedocs.org/projects/predictable/badge/?version=latest)](https://predictable.readthedocs.io/en/latest/?badge=latest)

## What is it?

A framework for actuarial modelling.

## Installation

```sh
pip install predictable
```

## Quick start example

A `model.py` file will be used to house the modelling logic which will be applied to each modelpoint.

```python
# import the library
from predictable import CashFlow, DiscountFactors, Model, StaticCashFlow

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
    StaticCashFlow(
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

## Documentation

This project is documented using sphinx and the full documentation can be found at [predictable.readthedocs.io](https://predictable.readthedocs.io/en/latest/).

## Development & Contibutions

The following steps can be followed to set up a development environment.

1. Clone the project:

    ```sh
    git clone https://github.com/RatulMaharaj/predictable.git
    cd predictable
    ```

2. Install [hatch](https://hatch.pypa.io/latest/)

    ```sh
    pipx install hatch
    ```

3. Enter the default environment (this will activate the default virtual environment and install the project in editable mode).

    ```sh
    hatch shell default
    ```

### Testing

This project uses `pytest` for testing purposes. The tests can be found in the `tests` directory. Tests will run after every commit (locally) and on every push (using github actions) but can also be run manually using:

```sh
hatch run test
```

### Linting

This project is linted using `ruff` and formatted with `black`. The linting and formatting can be run manually using:

```sh
hatch run lint
```

```sh
hatch run format
```

### Editing the docs

The documentation for this project can be found in the `docs` directory. The documentation is built using sphinx and can be built locally using:

```sh
hatch run docs:make
```

You can then serve the documentation locally using:

```sh
hatch run docs:serve
```

## License

[MIT](https://github.com/RatulMaharaj/predictable/blob/main/LICENSE)
