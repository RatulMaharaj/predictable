"""predictable, driven exactly as a user drives it: `predictable run run.pir`.

The subprocess is deliberate. Timing the CLI end to end includes parse, check, plan, table
build, modelpoint load and the Parquet write — the costs a library-level harness hides and
the ones that dominate at `M = 1`. `03-engine.md` §11.4 asks for time-to-first-result at
`M = 1`; you only get that number honestly by paying for process start too.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

import numpy as np
import pyarrow.parquet as pq

from ..outputs import Outputs

NAME = "predictable"

ROOT = Path(__file__).resolve().parents[3]
BINARY = ROOT / "target" / "release" / "predictable"


def available() -> bool:
    return BINARY.exists()


def version() -> str:
    out = subprocess.run([str(BINARY), "--version"], capture_output=True, text=True)
    return (out.stdout or out.stderr).strip()


def execute(work: Path, *, threads: int = 1, out: str = "runs/bench") -> dict:
    """Run the model. Returns the parsed manifest of the completed run."""
    run_dir = work / out
    # Create the output directory ourselves rather than relying on the CLI to do it: the
    # harness reuses a work directory across measurements, and a run that has to create a
    # nested path it did not create is one more thing that can fail between two timings.
    run_dir.mkdir(parents=True, exist_ok=True)
    cmd = [
        str(BINARY),
        "run",
        "run.pir",
        "--out",
        out,
        "--threads",
        str(threads),
    ]
    proc = subprocess.run(cmd, cwd=work, capture_output=True, text=True)
    if proc.returncode != 0:
        raise RuntimeError(f"predictable run failed ({proc.returncode}):\n{proc.stderr}")
    return json.loads((run_dir / "manifest.json").read_text())


def collect(work: Path, *, out: str = "runs/bench") -> Outputs:
    """Read `results.parquet` back into the comparison contract."""
    table = pq.read_table(work / out / "results.parquet").to_pydict()
    keys, comps, ts, vals = (
        table["mp_key"],
        table["component"],
        table["t"],
        table["value"],
    )
    order: dict[str, int] = {}
    for k in keys:
        if k not in order:
            order[k] = len(order)
    m = len(order)

    per_mp: dict[str, np.ndarray] = {}
    series: dict[str, dict[int, np.ndarray]] = {}
    for key, comp, t, v in zip(keys, comps, ts, vals):
        name = comp.split(".")[-1]
        # `t = -1` is how the writer marks an untimed `PerMP` row (`01-ir.md` §2.5).
        if t is None or t < 0:
            per_mp.setdefault(name, np.full(m, np.nan))[order[key]] = v
        else:
            series.setdefault(name, {}).setdefault(t, np.full(m, np.nan))[order[key]] = v

    stacked = {}
    for name, by_t in series.items():
        stacked[name] = np.stack([by_t[t] for t in sorted(by_t)], axis=1)
    return Outputs(keys=list(order), per_mp=per_mp, series=stacked)
