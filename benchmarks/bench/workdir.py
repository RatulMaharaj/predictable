"""Build a self-contained run directory for one scenario at one portfolio size.

The committed reference model — its `.pir` build, its `base.pir`, its tables — is copied
verbatim; only `data/modelpoints.csv` is regenerated at size `M`. That is what makes the
benchmark a benchmark of the *engine* rather than of a benchmark-specific model: the
`model_digest` of a run here is byte-identical to the one in `models/<name>/expected/`.
"""

from __future__ import annotations

import shutil
from pathlib import Path

from .basis import (
    MODELS,
    SAVINGS_HEADER,
    TERM_HEADER,
    savings_modelpoints,
    term_modelpoints,
    write_modelpoints,
)

TERM_LIKE = {"term_annual", "term_monthly", "term_solve"}


def modelpoints(scenario: str, size: int) -> list[dict]:
    if scenario in TERM_LIKE:
        return term_modelpoints(size)
    return savings_modelpoints(size)


def prepare(scenario: str, size: int, root: Path) -> Path:
    """Materialise `<root>/<scenario>-<size>/` and return it."""
    src = MODELS / scenario
    dst = root / f"{scenario}-{size}"
    if dst.exists():
        shutil.rmtree(dst)
    dst.mkdir(parents=True)
    shutil.copytree(src / "build", dst / "build")
    shutil.copytree(src / "tables", dst / "tables")
    shutil.copy(src / "base.pir", dst / "base.pir")
    shutil.copy(src / "run.pir", dst / "run.pir")
    header = TERM_HEADER if scenario in TERM_LIKE else SAVINGS_HEADER
    write_modelpoints(dst / "data" / "modelpoints.csv", header, modelpoints(scenario, size))
    return dst
