"""`skills/predictable-migration/scripts/loop.py`, driven over the real walkthrough scenario.

The skill's loop is a claim about behaviour, so it is tested as behaviour: the seeded
off-by-one from `docs/v2/verification-walkthrough.md` is rebuilt in a temporary directory,
the loop is run over it exactly as the skill instructs an agent to run it, and the report
is asserted on.

What is asserted, and why each one matters to the skill:

1. **The loop localises.** `roots` contains the component that was actually edited —
   `premium_rate` — and not the six outputs that visibly moved. If this ever regresses the
   skill's step 6 ("work the findings top-down") is advice with nothing behind it.
2. **The stop condition is honest.** A run whose numbers match but whose components carry
   no `meta.source` reports `done = false`, because coverage is a completion criterion in
   its own right (04-verify.md §8.2 step 4/7).
3. **The prohibition is enforced, not merely documented.** Asking for a wider absolute or
   relative tolerance is refused with exit code 3 before anything is executed.
4. **Every diagnostic code the report surfaces is resolvable**, which is the property the
   diagnostics index and its CI link check exist to guarantee.

Run:  `pytest models/tests -q`  (needs `cargo build -p predictable-cli` first).
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

MODELS_DIR = Path(__file__).resolve().parent.parent
REPO = MODELS_DIR.parent
LOOP = REPO / "skills" / "predictable-migration" / "scripts" / "loop.py"
sys.path.insert(0, str(MODELS_DIR / "tools"))

import regen  # noqa: E402


@pytest.fixture(scope="session")
def cli() -> Path:
    try:
        return regen.cli_binary()
    except SystemExit as exc:  # pragma: no cover - environment guard
        pytest.skip(str(exc))


def _seed_scenario(root: Path) -> Path:
    """`good/` and `bad/`, differing by the walkthrough's one-line off-by-one."""
    for side in ("good", "bad"):
        shutil.copytree(MODELS_DIR / "term_annual", root / side, dirs_exist_ok=True)
        shutil.rmtree(root / side / "runs", ignore_errors=True)
        base = root / side / "base.pir"
        base.write_text(
            base.read_text().replace("premium_escalation = 0.0", "premium_escalation = 0.03"),
            encoding="utf-8",
        )
        # `emit = "all"`: the diff cannot localise to a component it was not given.
        run_pir = root / side / "run.pir"
        run_pir.write_text(
            run_pir.read_text().replace('emit = "outputs"', 'emit = "all"'), encoding="utf-8"
        )
    model = root / "bad" / "build" / "model.pir"
    model.write_text(
        model.read_text().replace(
            'init = "annual_premium"', 'init = "annual_premium * (1 + premium_escalation)"'
        ),
        encoding="utf-8",
    )
    return root


def _loop(cli: Path, cwd: Path, *args: str) -> tuple[int, dict]:
    done = subprocess.run(
        [sys.executable, str(LOOP), "--bin", str(cli), "--cwd", str(cwd), *args],
        capture_output=True,
        text=True,
    )
    assert done.stdout.strip(), f"loop.py printed nothing\n{done.stderr}"
    return done.returncode, json.loads(done.stdout)


@pytest.fixture(scope="session")
def scenario(tmp_path_factory) -> Path:
    return _seed_scenario(tmp_path_factory.mktemp("walkthrough"))


@pytest.fixture(scope="session")
def baseline(cli, scenario) -> Path:
    done = subprocess.run(
        [str(cli), "run", "run.pir", "--out", "runs/all", "--retain-all"],
        cwd=scenario / "good",
        capture_output=True,
        text=True,
    )
    assert done.returncode == 0, done.stderr
    return scenario / "good" / "runs" / "all"


@pytest.fixture(scope="session")
def diverged_report(cli, scenario, baseline) -> tuple[int, dict]:
    return _loop(
        cli,
        scenario,
        "--model", "bad/build",
        "--model", "bad/base.pir",
        "--run", "bad/run.pir",
        "--baseline", "good/runs/all",
        "--baseline-model", "good/build",
        "--out", "bad/runs/loop",
    )


def test_loop_localises_the_seeded_bug(diverged_report):
    code, report = diverged_report
    assert code == 1, report  # unfinished, not blocked
    assert report["verdict"] == "diverged"
    assert report["done"] is False
    roots = [r["component"] for r in report["roots"]]
    assert roots == ["model.premium_rate"], report["roots"]
    root = report["roots"][0]
    assert root["class_basis"] == "ir_graph"
    assert root["explained_by_model_change"]["changed"] is True
    assert "init" in root["explained_by_model_change"]["what"]
    # Six components moved; exactly one is a root and the rest are reported as inherited.
    assert report["inherited_findings"] >= 5


def test_loop_reports_hypotheses_and_a_next_action(diverged_report):
    _, report = diverged_report
    codes = {h["code"] for h in report["roots"][0]["hypotheses"]}
    assert {"H0101", "H0201"} <= codes
    high = [h for h in report["roots"][0]["hypotheses"] if h["confidence"] == "high"]
    assert high, report["roots"][0]["hypotheses"]
    assert any("suggested edit" in a for a in report["next_actions"])
    # Step 6 is top-down: no action is proposed for an inherited finding.
    assert all("premium_rate" in a or "meta.source" in a or "W0102" in a for a in report["next_actions"])


def test_every_step_of_the_loop_is_recorded(diverged_report):
    _, report = diverged_report
    assert [s["step"] for s in report["steps"]] == ["check", "run", "diff"]
    run_step = report["steps"][1]
    assert "--retain-all" in run_step["command"]  # the emit setting is part of the answer
    assert run_step["traps"] == []
    assert report["steps"][2]["graph_available"] is True


def test_coverage_is_a_completion_criterion(diverged_report):
    _, report = diverged_report
    coverage = report["coverage"]
    assert coverage["components"] == 22
    # The reference model is not a migration, so nothing carries `meta.source`; the loop
    # must say so rather than call the migration finished on numbers alone.
    assert coverage["with_source"] == 0
    assert coverage["complete"] is False
    assert report["stop_condition"]["coverage_complete"] is False


def test_matching_runs_are_still_not_done_without_source_coverage(cli, scenario, baseline):
    code, report = _loop(
        cli,
        scenario,
        "--model", "good/build",
        "--model", "good/base.pir",
        "--run", "good/run.pir",
        "--baseline", "good/runs/all",
        "--baseline-model", "good/build",
        "--out", "good/runs/loop",
    )
    assert report["verdict"] == "matched"
    assert report["stop_condition"]["matched"] is True
    assert report["stop_condition"]["check_clean"] is True
    assert report["done"] is False and code == 1
    assert any("meta.source" in a for a in report["next_actions"])


def test_source_coverage_counts_meta_source(cli, scenario, tmp_path):
    """A component with `[component.meta.source]` counts; one without does not."""
    sys.path.insert(0, str(LOOP.parent))
    import loop as loop_module  # noqa: PLC0415

    model = scenario / "bad" / "build" / "model.pir"
    text = model.read_text().replace(
        'name = "age"\nkind = "Derived"',
        'name = "age"\nkind = "Derived"',
    )
    annotated = tmp_path / "annotated.pir"
    annotated.write_text(
        text.replace(
            'expr = "entry_age + t"',
            'expr = "entry_age + t"\n\n[component.meta.source]\nsystem = "prophet"\n'
            'variable = "ATT_AGE"\nfile = "TERM.MOD:41"',
            1,
        ),
        encoding="utf-8",
    )
    coverage = loop_module.source_coverage(str(tmp_path), ["annotated.pir"])
    assert coverage["with_source"] == 1
    assert coverage["complete"] is False
    assert "model.age" not in coverage["missing_source"]


@pytest.mark.parametrize("flag", ["--abs", "--rel"])
def test_widening_a_tolerance_is_refused(cli, scenario, flag):
    """The hard prohibition of 04-verify.md §8.2 step 6, enforced before anything runs."""
    code, report = _loop(
        cli,
        scenario,
        "--model", "bad/build",
        "--run", "bad/run.pir",
        "--baseline", "good/runs/all",
        flag, "1000.0",
    )
    assert code == 3
    assert report["verdict"] == "refused"
    assert report["reason"] == "tolerance_override_requested"
    assert "prohibited" in report["message"]
    assert report["done"] is False


def test_checker_errors_block_the_loop(cli, scenario, tmp_path):
    """Schema first: a model that does not check never reaches the diff."""
    broken = tmp_path / "broken"
    shutil.copytree(scenario / "bad", broken)
    model = broken / "build" / "model.pir"
    model.write_text(
        model.read_text().replace('expr = "entry_age + t"', 'expr = "no_such_name + t"'),
        encoding="utf-8",
    )
    code, report = _loop(
        cli,
        broken,
        "--model", "build",
        "--run", "run.pir",
        "--baseline", str(scenario / "good" / "runs" / "all"),
        "--out", "runs/loop",
    )
    assert code == 2
    assert report["verdict"] == "blocked"
    assert [s["step"] for s in report["steps"]] == ["check"]
    assert report["steps"][0]["errors"] > 0
    # Every code the agent is handed resolves to the diagnostics catalogue.
    catalogue = (REPO / "docs" / "llm" / "diagnostics.md").read_text(encoding="utf-8")
    for diagnostic in report["diagnostics"]:
        assert f'<a id="{diagnostic["code"]}"></a>' in catalogue
        assert diagnostic["doc_url"].endswith("#" + diagnostic["code"])
