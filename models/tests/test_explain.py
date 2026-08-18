"""`explain()` against the reference corpus (`04-verify.md` §3).

One claim is being tested here, over and over, on real models: **the replay is
the run**. `predictable explain` re-projects a single modelpoint through a fresh
single-lane engine and reports the tree behind one cell; if the tree's arithmetic
did not reproduce the vectorised kernel's stored value, the engine would raise
`E0901` and the command would exit 1.

So each test does the same two things: it runs the model, and then it explains
cells out of that run and compares the trace's value with the value that is
actually in `results.parquet`. Anything less would be testing `explain` against
itself.

Run:  `pytest models/tests -q`  (needs `cargo build -p predictable-cli` first).
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest

MODELS_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(MODELS_DIR / "tools"))

import goldens  # noqa: E402
import regen  # noqa: E402

pytest.importorskip("pyarrow")

MODELS = regen.MODELS


@pytest.fixture(scope="session")
def cli() -> Path:
    try:
        return regen.cli_binary()
    except SystemExit as exc:  # pragma: no cover - environment guard
        pytest.skip(str(exc))


@pytest.fixture(scope="session")
def runs(cli, tmp_path_factory) -> dict[str, Path]:
    """Every model, run once. The committed `runs/` is not used."""
    out: dict[str, Path] = {}
    for model in MODELS:
        target = tmp_path_factory.mktemp(model) / "run"
        done = regen.run(model, out=target)
        assert done.returncode == 0, f"{model}: run failed\n{done.stdout}\n{done.stderr}"
        out[model] = target
    return out


def explain(cli: Path, model: str, run_dir: Path, component: str, mp: str, t=None, **flags):
    """`predictable explain --json`, parsed. Runs in the model directory, which is
    where the run itself was launched from and therefore where its relative paths
    resolve."""
    argv = [
        str(cli),
        "explain",
        str(run_dir),
        "--json",
        "--component",
        component,
        "--mp",
        mp,
    ]
    if t is not None:
        argv += ["--t", str(t)]
    for key, value in flags.items():
        argv += [f"--{key.replace('_', '-')}"] + ([str(value)] if value is not True else [])
    done = subprocess.run(
        argv, cwd=str(MODELS_DIR / model), capture_output=True, text=True
    )
    assert done.returncode == 0, (
        f"{model}: explain {component} {mp} t={t} exited {done.returncode}\n"
        f"{done.stdout}\n{done.stderr}"
    )
    return json.loads(done.stdout)


def index(rows) -> dict:
    """{(mp_key, component, t): value} — the run's own numbers."""
    return {(r[0], r[2], r[3]): r[4] for r in rows}


def shapes(rows) -> tuple[list[str], list[str], list[str]]:
    """(modelpoint keys, series components, per-mp components) present in a run."""
    keys, series, permp = [], [], []
    for mp_key, _row, component, t, _value in rows:
        if mp_key not in keys:
            keys.append(mp_key)
        bucket = permp if t == -1 else series
        if component not in bucket:
            bucket.append(component)
    return keys, series, permp


def walk(node):
    yield node
    for child in node.get("children", []):
        yield from walk(child)


# ---------------------------------------------------------------------------------------
# 1. replay == run, on every reference model
# ---------------------------------------------------------------------------------------


@pytest.mark.parametrize("model", MODELS)
def test_every_permp_output_replays_to_the_value_the_run_stored(cli, model, runs):
    rows = goldens.read_results(runs[model])
    values = index(rows)
    keys, _series, permp = shapes(rows)
    mp = keys[0]
    assert permp, f"{model}: no PerMP outputs to explain"
    for component in permp:
        doc = explain(cli, model, runs[model], component, mp)
        # The document *is* the trace of §3.2, not a wrapper around one.
        assert doc["format"] == "pvf/1" and doc["kind"] == "trace"
        assert doc["root"]["value"] == values[(mp, component, -1)], (
            f"{model}: replay of {component} for {mp} diverged from the run"
        )


@pytest.mark.parametrize("model", MODELS)
def test_series_outputs_replay_at_several_periods(cli, model, runs):
    rows = goldens.read_results(runs[model])
    values = index(rows)
    keys, series, _permp = shapes(rows)
    mp = keys[0]
    periods = max(t for (_k, _c, t) in values)
    # Beginning, the recursion's second step, the middle, and the last period:
    # the four places a lag, an `init` and a run-off boundary can go wrong.
    sample = sorted({0, 1, periods // 2, periods})
    for component in series[:3]:
        for t in sample:
            doc = explain(cli, model, runs[model], component, mp, t)
            assert doc["root"]["t"] == t
            assert doc["root"]["value"] == values[(mp, component, t)], (
                f"{model}: replay of {component}[t={t}] for {mp} diverged from the run"
            )


def test_bel_replays_for_every_modelpoint_of_term_annual(cli, runs):
    """The headline case of `04-verify.md` §3: `explain bel` for a whole portfolio."""
    rows = goldens.read_results(runs["term_annual"])
    values = index(rows)
    keys, _series, _permp = shapes(rows)
    for mp in keys:
        doc = explain(cli, "term_annual", runs["term_annual"], "bel", mp)
        assert doc["root"]["value"] == values[(mp, "model.bel", -1)], mp


def test_the_solved_projection_is_what_gets_replayed(cli, runs):
    """`term_solve` writes the solved `vary` value back into its modelpoints. A
    replay of the file on disk would explain a projection nobody ran, so the
    trace's value must equal the *solved* run's."""
    rows = goldens.read_results(runs["term_solve"])
    values = index(rows)
    keys, _series, permp = shapes(rows)
    mp = keys[0]
    for component in permp:
        doc = explain(cli, "term_solve", runs["term_solve"], component, mp)
        assert doc["root"]["value"] == values[(mp, component, -1)], component
    # The solved BEL is zero by construction, and the trace says so.
    assert values[(mp, "model.bel", -1)] == pytest.approx(0.0, abs=1e-6)


# ---------------------------------------------------------------------------------------
# 2. the tree is a real provenance trace, not a formula dump
# ---------------------------------------------------------------------------------------


def test_a_full_trace_bottoms_out_in_inputs_and_literals(cli, runs):
    doc = explain(cli, "term_annual", runs["term_annual"], "reserve", "TA00001", 4, depth=-1)
    nodes = list(walk(doc["root"]))
    leaves = [n for n in nodes if not n.get("children")]
    assert leaves
    assert {n["node"] for n in leaves} <= {"Input", "Lit", "Agg", "Lookup"}
    # And the inputs name where they came from.
    inputs = [n for n in nodes if n["node"] == "Input"]
    assert any(n["kind"] == "Modelpoint" for n in inputs)
    assert any(n["kind"] == "Assumption" for n in inputs)


def test_npv_terms_are_present_ordered_and_sum_exactly(cli, runs):
    doc = explain(cli, "term_annual", runs["term_annual"], "bel", "TA00001", depth=-1)
    aggs = [n for n in walk(doc["root"]) if n["node"] == "Agg"]
    assert aggs, "a BEL is an aggregate; its terms are the point of the trace"
    for agg in aggs:
        terms = agg["terms"]
        assert [term["t"] for term in terms] == list(range(len(terms))), "ascending, no gaps"
        total = 0.0
        for term in terms:
            total += term["contribution"]
        assert total == agg["value"], f"{agg['op']} contributions must sum exactly"
        if agg["op"] == "npv":
            # The discount exponent is the bug; it must be visible as data.
            assert all("disc" in term for term in terms)
            assert agg["over"]["exponent_rule"] in ("v^t", "v^(t+1)", "v^(t+0.5)")


def test_a_lookup_reports_its_keys_the_table_and_the_policy(cli, runs):
    doc = explain(cli, "term_annual", runs["term_annual"], "qx", "TA00001", 3, depth=-1)
    lookups = [n for n in walk(doc["root"]) if n["node"] == "Lookup"]
    assert lookups, "qx is a table lookup"
    for node in lookups:
        assert node["table"]
        assert node["table_digest"].startswith("sha256:")
        assert node["keys"], "every key is reported with the policy that fired"
        for key in node["keys"]:
            assert key["policy"] in ("exact", "clamp", "step", "interpolate")
            assert "requested" in key and "resolved" in key


def test_depth_shrinks_the_tree_and_never_the_number(cli, runs):
    shallow = explain(cli, "term_annual", runs["term_annual"], "reserve", "TA00002", 6, depth=1)
    full = explain(cli, "term_annual", runs["term_annual"], "reserve", "TA00002", 6, depth=-1)
    assert shallow["root"]["value"] == full["root"]["value"]
    assert len(list(walk(shallow["root"]))) < len(list(walk(full["root"])))
    assert shallow["truncated"]["elided_nodes"] > 0


def test_the_text_rendering_is_deterministic_and_matches_the_json(cli, runs):
    argv_common = ["explain", str(runs["term_annual"]), "--component", "bel", "--mp", "TA00001"]
    cli_path = str(regen.cli_binary())
    cwd = str(MODELS_DIR / "term_annual")

    def text():
        done = subprocess.run(
            [cli_path] + argv_common, cwd=cwd, capture_output=True, text=True
        )
        assert done.returncode == 0, done.stderr
        return done.stdout

    first = text()
    assert first == text(), "the same tree must render identically every time"
    doc = explain(cli, "term_annual", runs["term_annual"], "bel", "TA00001")
    # The header names the component and the modelpoint, exactly as the JSON does.
    assert doc["root"]["id"] in first
    assert doc["root"]["modelpoint"]["key"] in first


# ---------------------------------------------------------------------------------------
# 3. the command's contract
# ---------------------------------------------------------------------------------------


def test_a_series_needs_a_t_and_a_permp_refuses_one(cli, runs):
    for argv, expected in (
        (["--component", "bel", "--mp", "TA00001", "--t", "3"], "not a Series"),
        (["--component", "reserve", "--mp", "TA00001"], "needs a `t`"),
        (["--component", "nope", "--mp", "TA00001"], "no component named"),
        (["--component", "bel", "--mp", "NOPE"], "no modelpoint"),
    ):
        done = subprocess.run(
            [str(cli), "explain", str(runs["term_annual"])] + argv,
            cwd=str(MODELS_DIR / "term_annual"),
            capture_output=True,
            text=True,
        )
        assert done.returncode == 2, f"{argv}: expected a usage failure"
        assert expected in done.stderr, f"{argv}: {done.stderr}"


def test_explain_is_no_longer_a_stub(cli):
    done = subprocess.run([str(cli), "--help"], capture_output=True, text=True)
    assert done.returncode == 0
    assert "explain <run/>" in done.stdout
    assert "explain | migrate" not in done.stdout
