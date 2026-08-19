"""Behavioural tests for the three launch demos in `demos/`.

The demos are the project's public claims, so these tests check the claims rather than the
prose: that the synthetic Prophet workspace is reproducible from its generator, that the
inventory accounts for every Prophet variable, and — when a release CLI is available — that
each `run_demo.sh` actually runs green end to end.

The end-to-end tests are skipped, not failed, when `target/release/predictable` is missing,
because a source checkout without a build is a normal state and a skipped test says so more
usefully than a red one.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
DEMOS = ROOT / "demos"
MIGRATION = DEMOS / "migration"
IFRS17 = DEMOS / "ifrs17"
BENCHMARKS = DEMOS / "benchmarks"
CLI = ROOT / "target" / "release" / "predictable"

needs_cli = pytest.mark.skipif(not CLI.exists(), reason="build with: cargo build --release -p predictable-cli")


def _python() -> str:
    venv = ROOT / ".venv" / "bin" / "python"
    return str(venv) if venv.exists() else sys.executable


def _env() -> dict[str, str]:
    env = dict(os.environ)
    env["PYTHON"] = _python()
    env["PREDICTABLE"] = str(CLI)
    return env


# --------------------------------------------------------------------------- structure


@pytest.mark.parametrize("demo", ["migration", "benchmarks", "ifrs17"])
def test_every_demo_has_a_script_and_a_readme(demo: str) -> None:
    script = DEMOS / demo / "run_demo.sh"
    assert script.exists(), f"{demo} has no run_demo.sh"
    assert os.access(script, os.X_OK), f"{demo}/run_demo.sh is not executable"
    assert (DEMOS / demo / "README.md").read_text().strip(), f"{demo} has an empty README"


@pytest.mark.parametrize(
    "page",
    ["v2/demo-migration.md", "v2/demo-benchmarks.md", "v2/ifrs17-valuation.md"],
)
def test_every_demo_has_a_docs_page_in_the_nav(page: str) -> None:
    assert (ROOT / "docs" / page).exists()
    nav = (ROOT / "mkdocs.yml").read_text()
    assert page in nav, f"{page} is not reachable from the mkdocs nav"


def test_generated_demo_output_is_git_ignored() -> None:
    """A 600 MB run directory must never be committable by accident."""
    ignored = (DEMOS / ".gitignore").read_text()
    for path in ("ifrs17/runs/", "ifrs17/out/", "migration/runs/"):
        assert path in ignored


# ------------------------------------------------------- demo 1: the Prophet workspace


def test_the_prophet_workspace_is_reproducible_from_its_generator(tmp_path: Path) -> None:
    """The committed `.rpt`/`.mpf`/`.fac` must be exactly what the generator emits.

    This is what makes the demo's headline claim checkable: the reconciliation target is a
    deterministic function of a transcription a reader can audit, not a file of unknown
    provenance that happens to agree with us.
    """
    work = tmp_path / "migration"
    shutil.copytree(MIGRATION, work, ignore=shutil.ignore_patterns("runs", "build*", "__pycache__"))
    before = {p.name: p.read_bytes() for p in (work / "prophet").glob("*")}

    proc = subprocess.run(
        [_python(), str(work / "tools" / "make_prophet_run.py")],
        capture_output=True,
        text=True,
    )
    assert proc.returncode == 0, proc.stderr

    after = {p.name: p.read_bytes() for p in (work / "prophet").glob("*")}
    assert set(before) == set(after)
    for name in sorted(before):
        assert before[name] == after[name], f"{name} is not reproducible from the generator"


def test_the_prophet_inputs_convert_to_the_committed_predictable_inputs(tmp_path: Path) -> None:
    work = tmp_path / "migration"
    shutil.copytree(MIGRATION, work, ignore=shutil.ignore_patterns("runs", "build*", "__pycache__"))
    proc = subprocess.run(
        [_python(), str(work / "tools" / "import_prophet_inputs.py")],
        capture_output=True,
        text=True,
    )
    assert proc.returncode == 0, proc.stderr
    for rel in ("data/modelpoints.csv", "tables/mortality.csv", "tables/lapses.csv",
                "tables/expenses.csv"):
        assert (work / rel).read_bytes() == (MIGRATION / rel).read_bytes(), rel


def test_every_prophet_variable_is_accounted_for() -> None:
    """Step 1 of the migration skill: nothing may be silently absent."""
    mod = (MIGRATION / "prophet" / "TERM_UK.MOD").read_text()
    declared = {
        m.group(1)
        for m in re.finditer(r"^([A-Z][A-Z0-9_]*)\s*=", mod, re.MULTILINE)
    }
    # Run parameters are assumptions, not variables; the inventory records them separately.
    coverage = json.loads((MIGRATION / "migration" / "coverage.json").read_text())
    params = set(coverage["run_parameters"])
    variables = declared - params

    accounted = set()
    for bucket in coverage["variables"].values():
        accounted |= {entry["prophet"] for entry in bucket}

    assert variables - accounted == set(), f"unaccounted Prophet variables: {variables - accounted}"
    # The inverse direction too: the inventory may not name anything the library does not
    # contain. Table names (`EXP_BAND`) are referenced rather than assigned, so the test is
    # "appears in the source", not "is assigned in the source".
    for name in sorted(accounted - variables):
        assert re.search(rf"\b{re.escape(name)}\b", mod), f"inventory names {name}, which TERM_UK.MOD does not contain"


def test_out_of_scope_variables_carry_a_reason() -> None:
    coverage = json.loads((MIGRATION / "migration" / "coverage.json").read_text())
    for entry in coverage["variables"]["out_of_scope"]:
        assert entry.get("why"), f"{entry['prophet']} is out of scope without a reason"


def test_the_mapping_asserts_nothing_about_prophets_conventions() -> None:
    """`sign`, `scale` and `timing_shift` are claims about the Prophet side.

    This migration reconciles without any of them, and the demo says so. If one is ever
    added, this test fails and whoever added it has to justify it in the transcript rather
    than slipping it past a reader.
    """
    mapping = tomllib.loads((MIGRATION / "migration" / "mapping.toml").read_text())
    assert mapping["period_base"] == 1
    for entry in mapping["component"]:
        assert entry.get("sign", 1) == 1, entry
        assert entry.get("scale", 1.0) == 1.0, entry
        assert entry.get("timing_shift", 0) == 0, entry
    for entry in mapping.get("unmapped", []):
        assert entry.get("reason"), entry


def test_iteration_one_is_kept_and_really_differs() -> None:
    """The transcript is only a transcript if the broken draft is still in the repository."""
    draft = (MIGRATION / "steps" / "iteration1" / "model.py").read_text()
    final = (MIGRATION / "model.py").read_text()
    assert draft != final
    assert "mortality(age, sex, smoker)\n" in draft and "mortality_loading" not in draft.split("def qx")[1].split("def ")[0]
    assert "compound(expense_inflation, t + 1)" in draft
    assert "compound(expense_inflation, t)" in final


# ------------------------------------------------------------------ end to end (slow)


@needs_cli
def test_the_migration_demo_runs_green() -> None:
    proc = subprocess.run(
        ["bash", str(MIGRATION / "run_demo.sh")],
        capture_output=True,
        text=True,
        env=_env(),
        timeout=900,
    )
    assert "MIGRATION DEMO: OK" in proc.stdout, proc.stdout[-4000:] + proc.stderr[-2000:]
    assert proc.returncode == 0
    assert "  FAIL" not in proc.stdout

    final = json.loads((MIGRATION / "migration" / "diff_final.json").read_text())
    assert final["summary"]["verdict"] == "matched"
    assert final["summary"]["cells"]["diverged"] == 0
    assert final["summary"]["cells"]["compared"] > 6000
    assert final["tolerance"]["loosened"] == 0

    first = json.loads((MIGRATION / "migration" / "diff_iter1.json").read_text())
    assert first["summary"]["verdict"] == "diverged"
    roots = {f["component"].split(".")[-1] for f in first["findings"] if f["class"] == "root"}
    assert {"qx", "renewal_expenses"} <= roots


@needs_cli
def test_the_ifrs17_demo_runs_green_on_a_small_portfolio() -> None:
    env = _env()
    env["N"] = "500"
    proc = subprocess.run(
        ["bash", str(IFRS17 / "run_demo.sh")],
        capture_output=True,
        text=True,
        env=env,
        timeout=900,
    )
    assert "IFRS 17 DEMO: OK" in proc.stdout, proc.stdout[-4000:] + proc.stderr[-2000:]
    assert proc.returncode == 0

    recs = json.loads((IFRS17 / "out" / "reconciliations.json").read_text())
    assert recs["all_ok"], [c for c in recs["checks"] if not c["ok"]]
    assert len(recs["checks"]) == 5


@needs_cli
def test_the_ifrs17_reconciliations_would_fail_if_the_roll_forward_did_not_close() -> None:
    """The reconciliations are a test, not a formality.

    Perturb one year's released CSM in the roll-forward routine's own arithmetic and the
    closure check must report a residual and fail. Without this, "five checks passed" is
    consistent with five checks that cannot fail.
    """
    sys.path.insert(0, str(IFRS17 / "tools"))
    try:
        import csm_rollforward as rf
    finally:
        sys.path.pop(0)

    balance = {t: 1000.0 * (0.99**t) for t in range(13)}
    release = {t: 5.0 for t in range(12)}
    rows = rf.rollforward(balance, release, 12, "CSM")
    assert len(rows) == 1
    honest = rows[0]["residual"]

    tampered = dict(release)
    tampered[3] = 5.0 + 1.0  # a pound that the balance never actually gave up
    bad = rf.rollforward(balance, tampered, 12, "CSM")[0]
    # One pound of phantom release moves the residual by that pound plus the interest the
    # balance would have accreted on it — the point being that it moves at all, and by more
    # than the one-penny closure tolerance.
    moved = abs(bad["residual"] - honest)
    assert moved == pytest.approx(1.0 * (1 + rf.LOCKED_IN_M), abs=1e-9), moved
    assert abs(bad["residual"]) > 0.01, "a one-pound error must break the closure tolerance"


@needs_cli
@pytest.mark.skipif(
    not (ROOT / "benchmarks" / ".venv" / "bin" / "python").exists(),
    reason="benchmarks/.venv is not set up; see demos/benchmarks/README.md",
)
def test_the_benchmark_gate_runs_green() -> None:
    """Only the gate: the timing run takes an hour and is not a unit test."""
    env = _env()
    env["GATE_ONLY"] = "1"
    proc = subprocess.run(
        ["bash", str(BENCHMARKS / "run_demo.sh")],
        capture_output=True,
        text=True,
        env=env,
        timeout=1800,
    )
    assert proc.returncode == 0, proc.stdout[-4000:] + proc.stderr[-2000:]
    assert "gate OK" in proc.stdout
    assert "FAIL" not in proc.stdout
