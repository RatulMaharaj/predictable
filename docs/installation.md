# Installation

## Package installation

Predictable can be installed using pip:

```bash
pip install predictable
```

## Creating a project

Once installed, the predictable CLI can be used to create a new project:

```sh
predictable new my_project
```

This will scaffold a new project in the `my_project` directory. The structure of this project will look similar to:

```sh
my_project/
├── __init__.py
├── data/
│   └── modelpoints.csv
├── model.py
├── output/
├── run.py
└── tables/
```

## Project structure

| File | Description |
| --- | --- |
| `__init__.py` | This file allows us to use python modules  |
| `data/modelpoints.csv` | Example modelpoint file used (as input) when the model is run |
| `model.py` | Contains the model logic |
| `output/` | Stores model results and artifacts |
| `run.py` | Used to orchestrate model runs |
| `tables/` | Stores table inputs used by the model |
