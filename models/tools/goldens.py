"""Turn a run directory into the committed golden files, and compare one against them.

A golden is three files, and each answers a different question:

`summary.json`
    The digests. `results_digest` is the whole answer in 64 hex characters: if it matches,
    every number in the run matches, bit for bit. `model_digest` and `run_digest` say
    *which* model and *which* configuration produced them, so a golden that drifts because
    the model changed is distinguishable from one that drifts because the engine changed.

`per_mp.csv`
    Every `PerMP` output for every modelpoint, at full `repr` precision. This is the file a
    reviewer reads: it is the valuation result, one row per policy per measure.

`series_<key>.csv`
    Every `Series` output for one representative modelpoint, one row per `t`. This is the
    file that localises a break — a digest tells you *that* something moved, this tells you
    *when*.

Full precision matters more than readability here: these are equality goldens, not report
tables, so values are written with `repr(float)`, which round-trips exactly.
"""

from __future__ import annotations

import json
from pathlib import Path

import pyarrow.parquet as pq

#: Manifest fields that legitimately differ between two identical runs.
VOLATILE = ("run_id", "started_at", "finished_at", "wall_ms", "invocation")


def _num(value) -> str:
    if value is None:
        return ""
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, float):
        return repr(value)
    return str(value)


def read_results(run_dir: Path) -> list[tuple]:
    table = pq.read_table(run_dir / "results.parquet").to_pydict()
    cols = [table[c] for c in ("mp_key", "mp_row", "component", "t", "value")]
    return list(zip(*cols))


def summary(run_dir: Path) -> dict:
    manifest = json.loads((run_dir / "manifest.json").read_text())
    results = manifest["results"]
    out = {
        "model_digest": manifest["inputs"]["model"]["digest"],
        "assumption_digest": manifest["inputs"]["assumptions"]["digest"],
        "modelpoint_digest": manifest["inputs"]["modelpoints"]["digest"],
        "table_digests": {
            t["name"]: t["digest"] for t in manifest["inputs"].get("tables", []) or []
        },
        "run_digest": manifest["run_config"]["digest"],
        "results_digest": results["digest"],
        "component_set_digest": results["component_set_digest"],
        "rows": results["rows"],
        "components": results["components"],
        "outcome": manifest["execution"]["outcome"],
        "modelpoints_projected": manifest["execution"]["modelpoints_projected"],
        "periods": manifest["timeline"]["periods"],
    }
    if results.get("aggregates"):
        out["aggregates_digest"] = results["aggregates"]["digest"]
        out["aggregates_rows"] = results["aggregates"]["rows"]
    for solve in manifest["run_config"].get("solves", []) or []:
        out.setdefault("solves", {})[solve["name"]] = {
            "converged": solve.get("converged"),
            "not_converged": solve.get("not_converged"),
            "max_abs_residual": solve.get("residual", {}).get("max_abs"),
        }
    return out


def per_mp_csv(rows: list[tuple]) -> str:
    """One row per (modelpoint, PerMP component). `t = -1` is how the writer marks stage 2."""
    picked = sorted({(r[0], r[2], r[4]) for r in rows if r[3] == -1})
    lines = ["mp_key,component,value"]
    lines += [f"{k},{c},{_num(v)}" for k, c, v in picked]
    return "\n".join(lines) + "\n"


def series_csv(rows: list[tuple], mp_key: str) -> str:
    series = sorted({r[2] for r in rows if r[3] >= 0})
    by_t: dict[int, dict[str, object]] = {}
    for key, _row, comp, t, value in rows:
        if key == mp_key and t >= 0:
            by_t.setdefault(t, {})[comp] = value
    lines = ["t," + ",".join(series)]
    for t in sorted(by_t):
        lines.append(str(t) + "," + ",".join(_num(by_t[t].get(c)) for c in series))
    return "\n".join(lines) + "\n"


def aggregates_csv(run_dir: Path) -> str | None:
    path = run_dir / "aggregates.parquet"
    if not path.exists():
        return None
    table = pq.read_table(path).to_pydict()
    names = list(table)
    rows = list(zip(*[table[n] for n in names]))
    lines = [",".join(names)]
    lines += [",".join(_num(v) for v in row) for row in sorted(rows, key=lambda r: tuple(map(str, r)))]
    return "\n".join(lines) + "\n"


def solve_csv(path: Path) -> str:
    """`solves/<name>.parquet` verbatim, in `mp_row` order.

    The solved value, its residual and its iteration count are all reproducibility-relevant:
    a change in the root-finder that still converges to the same tolerance shows up here as
    a changed iteration count long before it shows up as a changed number.
    """
    table = pq.read_table(path).to_pydict()
    names = list(table)
    rows = sorted(zip(*[table[n] for n in names]), key=lambda r: r[names.index("mp_row")])
    return "\n".join([",".join(names)] + [",".join(_num(v) for v in row) for row in rows]) + "\n"


def build_goldens(run_dir: Path) -> dict[str, str]:
    """The golden file set for `run_dir`, as {relative name: text}."""
    rows = read_results(run_dir)
    first = min(r[0] for r in rows)
    files = {
        "summary.json": json.dumps(summary(run_dir), indent=2, sort_keys=True) + "\n",
        "per_mp.csv": per_mp_csv(rows),
        f"series_{first}.csv": series_csv(rows, first),
    }
    agg = aggregates_csv(run_dir)
    if agg is not None:
        files["aggregates.csv"] = agg
    for solve in sorted((run_dir / "solves").glob("*.parquet")):
        files[f"solve_{solve.stem}.csv"] = solve_csv(solve)
    return files


def write_goldens(run_dir: Path, out_dir: Path) -> list[str]:
    out_dir.mkdir(parents=True, exist_ok=True)
    files = build_goldens(run_dir)
    for name in sorted(p.name for p in out_dir.glob("series_*.csv")):
        if name not in files:
            (out_dir / name).unlink()
    for name, text in files.items():
        (out_dir / name).write_text(text, encoding="utf-8", newline="\n")
    return sorted(files)


def compare(run_dir: Path, out_dir: Path) -> list[str]:
    """Names of golden files the run disagrees with. Empty means the run reproduces."""
    bad = []
    for name, text in build_goldens(run_dir).items():
        path = out_dir / name
        if not path.exists():
            bad.append(f"{name}: missing from the committed goldens")
        elif path.read_text(encoding="utf-8") != text:
            bad.append(f"{name}: differs from the committed golden")
    return bad
