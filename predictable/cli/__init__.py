import click
from pathlib import Path
from .scaffold import model_code, run_code, modelpoints_csv
from predictable import __version__


@click.group(
    context_settings={"help_option_names": ["-h", "--help"]},
    invoke_without_command=True,
)
@click.version_option(__version__, message="%(version)s")
@click.pass_context
def cli(ctx):
    """Welcome to the predictable CLI"""
    pass


@cli.command()
@click.argument(
    "project_name", required=True, nargs=1, type=str, default="predictable_project"
)
@click.option(
    "--interactive", "-i", is_flag=True, help="Create a new project interactively."
)
def new(project_name, interactive):
    """Scaffold a new predictable project"""
    # get the current working directory
    cwd = Path.cwd()

    # prompt the user for a project name
    if interactive:
        proj_name = click.prompt(
            "Project name", type=str, default=project_name, show_default=False
        )
    else:
        proj_name = project_name

    # create a new directory for the project
    project_dir = cwd / proj_name
    project_dir.mkdir(exist_ok=True)

    # create the required subdirectories
    (project_dir / "data").mkdir(exist_ok=True)
    (project_dir / "tables").mkdir(exist_ok=True)

    # create the init file
    (project_dir / "__init__.py").touch()

    # write the required code to the files
    with open(project_dir / "data" / "modelpoints.csv", "w") as f:
        f.write(modelpoints_csv)

    with open(project_dir / "model.py", "w") as f:
        f.write(model_code)

    with open(project_dir / "run.py", "w") as f:
        f.write(run_code)

    click.echo(f"New project created successfully at {project_dir}")


@cli.command()
def run():
    """Trigger a predictable run"""
    click.echo("Finding and running run.py")
    run_py = Path.cwd() / "run.py"
    if run_py.exists():
        # TODO: not sure if this the best way to do this but it works for now
        exec(open(run_py).read())
    else:
        click.echo("No run.py file found in the current directory.")
