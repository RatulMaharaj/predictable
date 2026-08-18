"""Regenerate everything derived in `models/`: fixtures, `.pir` builds, runs, goldens.

    python models/tools/regen.py             # all five models
    python models/tools/regen.py term_annual # just one

The order is the dependency order and each step is a real invocation of the shipped tools,
not a shortcut through the library:

    1. `make_fixtures.py`   tables + modelpoint CSVs
    2. `predictable build`  model.py -> build/*.pir (this is what recomputes table digests)
    3. `predictable run`    run.pir -> runs/base/
    4. `goldens.write`      runs/base/ -> expected/

If step 2 changes a `.pir` the goldens must be regenerated too, which is why this is one
script rather than three. `models/tests/test_reference_models.py` runs the same steps in
`--check` mode and fails if anything here was not re-run.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

MODELS_DIR = Path(__file__).resolve().parent.parent
REPO = MODELS_DIR.parent

#: The reference corpus, in the order `03-engine.md` §11.2 lists it.
MODELS = ("term_annual", "term_monthly", "savings_monthly", "ifrs17_gmm", "term_solve")

DSL_SRC = REPO / "packages" / "predictable" / "src"


def cli_binary() -> Path:
    """The `predictable` engine CLI. Debug or release, whichever exists; debug wins."""
    for profile in ("debug", "release"):
        candidate = REPO / "target" / profile / "predictable"
        if candidate.exists():
            return candidate
    raise SystemExit("build the CLI first: cargo build -p predictable-cli")


def dsl_env() -> dict:
    """Environment for the Python DSL.

    `packages/predictable/src` goes on `PYTHONPATH` explicitly because the repository root
    still holds the v0 `predictable/` package, which would otherwise shadow it.
    """
    env = dict(os.environ)
    env["PYTHONPATH"] = str(DSL_SRC) + os.pathsep + env.get("PYTHONPATH", "")
    return env


def build(model: str, *, check: bool = False) -> subprocess.CompletedProcess:
    d = MODELS_DIR / model
    args = [sys.executable, "-m", "predictable", "build", str(d / "model.py"), "--out", str(d / "build")]
    if check:
        args.append("--check")
    # `cwd` is deliberately not the repo root: see `dsl_env`.
    return subprocess.run(args, cwd=str(MODELS_DIR), env=dsl_env(), capture_output=True, text=True)


def run(model: str, out: Path | None = None) -> subprocess.CompletedProcess:
    d = MODELS_DIR / model
    target = out or (d / "runs" / "base")
    return subprocess.run(
        [str(cli_binary()), "run", "run.pir", "--out", str(target)],
        cwd=str(d),
        capture_output=True,
        text=True,
    )


def main(argv: list[str]) -> int:
    sys.path.insert(0, str(MODELS_DIR / "tools"))
    import goldens
    import make_fixtures

    wanted = argv[1:] or list(MODELS)
    make_fixtures.main()

    for model in wanted:
        if model not in MODELS:
            print(f"unknown model {model!r}; known: {', '.join(MODELS)}", file=sys.stderr)
            return 2
        built = build(model)
        print(f"[{model}] build: {built.stdout.strip().splitlines()[-1] if built.stdout else built.returncode}")
        if built.returncode not in (0, 1):
            print(built.stdout, built.stderr, file=sys.stderr)
            return built.returncode
        done = run(model)
        if done.returncode != 0:
            print(done.stdout, done.stderr, file=sys.stderr)
            return done.returncode
        names = goldens.write_goldens(
            MODELS_DIR / model / "runs" / "base", MODELS_DIR / model / "expected"
        )
        print(f"[{model}] goldens: {', '.join(names)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
