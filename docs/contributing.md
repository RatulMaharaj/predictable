# Contributing

## Environment

The following steps can be followed to set up a development environment.

1. Clone the project and enter the directory:

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

You can now proceed to make changes to the project.

## Testing

This project uses `pytest` for testing purposes. The tests can be found in the `tests` directory. Tests will run after every commit (locally) and on every push (using github actions) but can also be run manually using:

```sh
hatch run test
```

## Linting

This project is linted using `ruff` and formatted with `black`. The linting and formatting can be run manually using:

```sh
hatch run lint
```

```sh
hatch run format
```

## Documentation

The documentation for this project can be found in the `docs` directory. The documentation is created using mkdocs and can be viewed locally using:

```sh
hatch run docs:serve
```

The docs can also be built for deployment using:

```sh
hatch run docs:build
```
