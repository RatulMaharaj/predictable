"""The reference corpus is only a corpus if it is checked.

Four things are asserted here, and they are different kinds of claim:

1. **The committed `.pir` is what the DSL produces.** `predictable build --check` fails if
   `model.py` and `build/*.pir` have drifted, so the two representations of every reference
   model cannot silently disagree.
2. **The committed goldens are what the runner produces.** Each model is re-run into a
   temporary directory and every golden file must match byte for byte.
3. **The run is invariant to how it was scheduled.** `run(C = 1) ≡ run(C = 4096)` and
   `run(threads = 1) ≡ run(threads = 4)`, bit for bit (`03-engine.md` §7).
4. **The numbers are actuarially coherent.** The identities every one of these models is
   supposed to satisfy — the BEL decomposition, `reserve[0] = bel`, `bel = 0` after the
   solve, IFRS 17's no-gain-at-initial-recognition — are asserted on the real output,
   because a golden that is internally consistent and actuarially wrong is worse than no
   golden at all.

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
sys.path.insert(0, str(MODELS_DIR / "tools"))

import goldens  # noqa: E402
import make_fixtures  # noqa: E402
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
def fresh_runs(cli, tmp_path_factory) -> dict[str, Path]:
    """Every model, run once into a temporary directory. The committed `runs/` is not used."""
    out: dict[str, Path] = {}
    for model in MODELS:
        target = tmp_path_factory.mktemp(model) / "run"
        done = regen.run(model, out=target)
        assert done.returncode == 0, f"{model}: run failed\n{done.stdout}\n{done.stderr}"
        out[model] = target
    return out


@pytest.fixture(scope="session")
def results(fresh_runs) -> dict[str, list[tuple]]:
    return {m: goldens.read_results(d) for m, d in fresh_runs.items()}


def _is(component_id: str, name: str) -> bool:
    """Match a bare component name against a qualified id.

    Model components are written as `<module>.<name>`; a solved value is materialised
    unqualified under the solve's own name (`01-ir.md` §8.4.4), so both forms must match.
    """
    return component_id == name or component_id.endswith("." + name)


def per_mp(rows: list[tuple], component: str) -> dict[str, float]:
    """{mp_key: value} for one `PerMP` component. Stage-2 rows are written at `t = -1`."""
    return {r[0]: r[4] for r in rows if r[3] == -1 and _is(r[2], component)}


def series_of(rows: list[tuple], mp_key: str, component: str) -> list[float]:
    picked = [(r[3], r[4]) for r in rows if r[0] == mp_key and _is(r[2], component) and r[3] >= 0]
    return [v for _t, v in sorted(picked)]


# ---------------------------------------------------------------------------------------
# 0. the corpus is complete and self-describing
# ---------------------------------------------------------------------------------------


@pytest.mark.parametrize("model", MODELS)
def test_every_model_ships_source_build_config_and_goldens(model):
    d = MODELS_DIR / model
    assert (d / "model.py").is_file(), "the DSL source"
    for name in ("schema.pir", "model.pir", "product.pir"):
        assert (d / "build" / name).is_file(), f"the committed canonical {name}"
    assert (d / "run.pir").is_file() and (d / "base.pir").is_file()
    assert (d / "data" / "modelpoints.csv").is_file()
    assert list((d / "tables").glob("*.csv")), "at least one table fixture"
    assert (d / "expected" / "summary.json").is_file()
    assert (d / "expected" / "per_mp.csv").is_file()


def test_fixtures_regenerate_byte_identically(tmp_path, monkeypatch):
    """`make_fixtures.py` is the fixtures' definition, so it must be reproducible."""
    before = {
        p.relative_to(MODELS_DIR): p.read_bytes()
        for model in MODELS
        for sub in ("tables", "data")
        for p in sorted((MODELS_DIR / model / sub).glob("*.csv"))
    }
    monkeypatch.setattr(make_fixtures, "ROOT", tmp_path)
    make_fixtures.main()
    after = {
        p.relative_to(tmp_path): p.read_bytes()
        for model in MODELS
        for sub in ("tables", "data")
        for p in sorted((tmp_path / model / sub).glob("*.csv"))
    }
    assert set(after) == set(before)
    assert after == before


# ---------------------------------------------------------------------------------------
# 1. the DSL and the committed .pir agree
# ---------------------------------------------------------------------------------------


@pytest.mark.parametrize("model", MODELS)
def test_committed_pir_is_up_to_date_with_the_dsl(model):
    done = regen.build(model, check=True)
    assert done.returncode == 0, (
        f"{model}: build/ has drifted from model.py — run "
        f"`python models/tools/regen.py {model}`\n{done.stdout}\n{done.stderr}"
    )


@pytest.mark.parametrize("model", MODELS)
def test_committed_pir_checks_clean(cli, model):
    """The `.pir` must pass the engine's own checker with no errors *and* no lints."""
    d = MODELS_DIR / model / "build"
    done = subprocess.run(
        [str(cli), "check", str(d / "schema.pir"), str(d / "model.pir"), str(d / "product.pir")],
        capture_output=True,
        text=True,
    )
    assert done.returncode == 0, f"{model}: check reported diagnostics\n{done.stdout}{done.stderr}"


def test_term_solve_is_the_same_model_as_term_annual():
    """`term_solve` differs from `term_annual` only in its run configuration.

    Equal `model_digest`, different `run_digest`: exactly the situation a run diff must
    attribute to the configuration rather than to the model.
    """
    a = json.loads((MODELS_DIR / "term_annual" / "expected" / "summary.json").read_text())
    b = json.loads((MODELS_DIR / "term_solve" / "expected" / "summary.json").read_text())
    assert a["model_digest"] == b["model_digest"]
    assert a["run_digest"] != b["run_digest"]


# ---------------------------------------------------------------------------------------
# 2. the goldens reproduce
# ---------------------------------------------------------------------------------------


@pytest.mark.parametrize("model", MODELS)
def test_run_reproduces_the_committed_goldens(fresh_runs, model):
    differences = goldens.compare(fresh_runs[model], MODELS_DIR / model / "expected")
    assert not differences, f"{model}: " + "; ".join(differences)


@pytest.mark.parametrize("model", MODELS)
def test_summary_matches_the_manifest_of_the_fresh_run(fresh_runs, model):
    committed = json.loads((MODELS_DIR / model / "expected" / "summary.json").read_text())
    assert goldens.summary(fresh_runs[model]) == committed


# ---------------------------------------------------------------------------------------
# 3. scheduling cannot change a number
# ---------------------------------------------------------------------------------------


def _values(run_dir: Path) -> dict:
    return {(r[0], r[2], r[3]): r[4] for r in goldens.read_results(run_dir)}


def _run(cli, model: str, out: Path, *flags: str) -> Path:
    shutil.rmtree(out, ignore_errors=True)
    done = subprocess.run(
        [str(cli), "run", "run.pir", "--out", str(out), *flags],
        cwd=str(MODELS_DIR / model),
        capture_output=True,
        text=True,
    )
    assert done.returncode == 0, f"{model} {flags}: {done.stdout}{done.stderr}"
    return out


def _digest_of(out: Path) -> str:
    return json.loads((out / "manifest.json").read_text())["results"]["digest"]


@pytest.mark.parametrize("model", ["term_annual", "ifrs17_gmm"])
def test_chunk_size_does_not_change_a_number(cli, tmp_path, model):
    """`run(C = 1) ≡ run(C = 4096)` on every value in the result set."""
    one = _run(cli, model, tmp_path / "c1", "--chunk-size", "1")
    many = _run(cli, model, tmp_path / "cN", "--chunk-size", "4096")
    assert _values(one) == _values(many)


@pytest.mark.parametrize("model", ["term_annual", "ifrs17_gmm"])
def test_thread_count_does_not_change_a_number(cli, tmp_path, model):
    serial = _run(cli, model, tmp_path / "t1", "--threads", "1", "--chunk-size", "4")
    parallel = _run(cli, model, tmp_path / "t4", "--threads", "4", "--chunk-size", "4")
    assert _values(serial) == _values(parallel)
    assert _digest_of(serial) == _digest_of(parallel)


@pytest.mark.parametrize("model", ["term_annual", "ifrs17_gmm"])
def test_aggregates_do_not_depend_on_the_chunking(cli, tmp_path, model):
    one = _run(cli, model, tmp_path / "a1", "--chunk-size", "1")
    many = _run(cli, model, tmp_path / "aN", "--chunk-size", "4096")
    left = json.loads((one / "manifest.json").read_text())["results"]["aggregates"]["digest"]
    right = json.loads((many / "manifest.json").read_text())["results"]["aggregates"]["digest"]
    assert left == right


@pytest.mark.parametrize("model", MODELS)
def test_chunk_size_does_not_change_the_result_bytes(cli, tmp_path, model):
    """`results.parquet` is byte-identical under any chunking, on every reference model.

    Values were always identical; the Parquet *encoding* used to move because the row-batch
    boundaries followed chunk arrival. The writer now drains exactly `batch_rows` rows per
    Arrow batch, so the physical layout is a function of the row sequence alone.
    """
    one = _run(cli, model, tmp_path / "b1", "--chunk-size", "1")
    many = _run(cli, model, tmp_path / "bN", "--chunk-size", "4096")
    odd = _run(cli, model, tmp_path / "b7", "--chunk-size", "7")
    assert _digest_of(one) == _digest_of(many) == _digest_of(odd)
    assert (
        (one / "results.parquet").read_bytes()
        == (many / "results.parquet").read_bytes()
        == (odd / "results.parquet").read_bytes()
    )


@pytest.mark.parametrize("model", MODELS)
def test_thread_count_does_not_change_the_result_bytes(cli, tmp_path, model):
    serial = _run(cli, model, tmp_path / "s1", "--threads", "1", "--chunk-size", "4")
    parallel = _run(cli, model, tmp_path / "s4", "--threads", "4", "--chunk-size", "4")
    assert (serial / "results.parquet").read_bytes() == (
        parallel / "results.parquet"
    ).read_bytes()


# ---------------------------------------------------------------------------------------
# 4. the numbers are actuarially coherent
# ---------------------------------------------------------------------------------------


@pytest.mark.parametrize("model", ["term_annual", "term_monthly", "term_solve"])
def test_term_bel_decomposition(results, model):
    """`BEL = PV(claims) + PV(expenses) + initial expense - PV(premiums)`, per modelpoint."""
    rows = results[model]
    bel = per_mp(rows, "bel")
    parts = {n: per_mp(rows, n) for n in ("pv_claims", "pv_expenses", "initial_expense", "pv_premiums")}
    assert bel
    for k, value in bel.items():
        expected = (
            parts["pv_claims"][k] + parts["pv_expenses"][k] + parts["initial_expense"][k]
            - parts["pv_premiums"][k]
        )
        assert value == pytest.approx(expected, rel=1e-12, abs=1e-9)


@pytest.mark.parametrize("model", ["term_annual", "term_monthly"])
def test_reserve_starts_at_the_bel(results, model):
    """`init = bel` is the stage-2 back-channel; `reserve[0]` must therefore *be* the BEL."""
    rows = results[model]
    bel = per_mp(rows, "bel")
    for k, value in bel.items():
        assert series_of(rows, k, "reserve")[0] == pytest.approx(value, rel=1e-12, abs=1e-9)


@pytest.mark.parametrize("model", ["term_annual", "term_monthly"])
def test_reserve_is_zero_after_the_term(results, model):
    rows = results[model]
    for k in per_mp(rows, "bel"):
        assert series_of(rows, k, "reserve")[-1] == 0.0


@pytest.mark.parametrize("model", ["term_annual", "term_monthly", "savings_monthly", "ifrs17_gmm"])
def test_decrement_counts_are_probabilities(results, model):
    """Deaths and surrenders are per policy sold, so they lie in [0, 1] and decay."""
    rows = results[model]
    for name in ("deaths", "surrenders"):
        values = [r[4] for r in rows if r[2].endswith("." + name) and r[3] >= 0]
        assert values
        assert min(values) >= 0.0
        assert max(values) <= 1.0


def test_monthly_and_annual_term_models_agree_in_shape(results):
    """The same product on two bases must give the same picture of the portfolio.

    The two are not equal and should not be: premiums are spread through the year rather
    than paid up front, and claims fall on average half a year earlier. The BEL itself is a
    small difference between large present values, so it is compared *relative to the
    premium base* rather than to itself — a 1% move in a PV is a large move in a BEL that
    happens to be near zero, and that is a fact about the product, not a defect.
    """
    annual, monthly = results["term_annual"], results["term_monthly"]
    pv_a, pv_m = per_mp(annual, "pv_premiums"), per_mp(monthly, "pv_premiums")
    cl_a, cl_m = per_mp(annual, "pv_claims"), per_mp(monthly, "pv_claims")
    bel_a, bel_m = per_mp(annual, "bel"), per_mp(monthly, "bel")

    assert set(pv_a) == set(pv_m)
    for k in pv_a:
        assert pv_m[k] == pytest.approx(pv_a[k], rel=0.10), "PV of premiums"
        assert cl_m[k] == pytest.approx(cl_a[k], rel=0.10), "PV of claims"
        assert abs(bel_m[k] - bel_a[k]) <= 0.10 * pv_a[k], "BEL, scaled by the premium base"

    # Ranking the portfolio by the size of the liability must not depend on the basis.
    assert sorted(cl_a, key=cl_a.get) == sorted(cl_m, key=cl_m.get)


def test_solve_drives_the_bel_to_zero(results, fresh_runs):
    """`term_solve` varies the premium until the BEL is zero, for every modelpoint."""
    for value in per_mp(results["term_solve"], "bel").values():
        assert value == pytest.approx(0.0, abs=1e-6)

    manifest = json.loads((fresh_runs["term_solve"] / "manifest.json").read_text())
    (solve,) = manifest["run_config"]["solves"]
    assert solve["not_converged"] == 0
    assert solve["converged"] == manifest["execution"]["modelpoints_projected"]
    assert abs(solve["residual"]["max_abs"]) < 1e-6


def test_solved_premium_is_materialised_as_a_component(results, fresh_runs):
    """`01-ir.md` §8.4.4: the solved value is also an ordinary `PerMP` component."""
    import pyarrow.parquet as pq

    solved = per_mp(results["term_solve"], "breakeven_premium")
    side = pq.read_table(fresh_runs["term_solve"] / "solves" / "breakeven_premium.parquet")
    side_values = dict(zip(side.column("mp_key").to_pylist(), side.column("solved_value").to_pylist()))
    assert solved == side_values
    assert all(1.0 <= v <= 100_000.0 for v in solved.values()), "inside the declared bracket"


def test_solved_premium_agrees_with_the_sign_of_the_unsolved_bel(results):
    """The solve and the base run must tell the same story about every policy.

    If `term_annual` says a policy has a negative BEL (it makes money at the office
    premium), then the premium that drives the BEL to zero must be *lower* than the office
    premium — and vice versa. This is the cross-model consistency check that would catch a
    solver converging to the wrong root or varying the wrong input.
    """
    import csv

    with (MODELS_DIR / "term_solve" / "data" / "modelpoints.csv").open() as handle:
        office = {r["policy_number"]: float(r["annual_premium"]) for r in csv.DictReader(handle)}
    solved = per_mp(results["term_solve"], "breakeven_premium")
    bel = per_mp(results["term_annual"], "bel")
    assert solved
    for k, value in solved.items():
        if bel[k] < 0:
            assert value < office[k], f"{k}: profitable, so break-even must be cheaper"
        elif bel[k] > 0:
            assert value > office[k], f"{k}: loss-making, so break-even must be dearer"


def test_savings_account_and_guarantee_behave(results):
    rows = results["savings_monthly"]
    for value in per_mp(rows, "pv_guarantee_cost").values():
        assert value >= 0.0, "a guarantee can never have negative value"
    biting = [v for v in per_mp(rows, "pv_guarantee_cost").values() if v > 0]
    assert biting, "the corpus is meant to contain policies where the guarantee bites"
    for value in per_mp(rows, "final_account_value").values():
        assert value >= 0.0


def test_savings_bel_decomposition(results):
    rows = results["savings_monthly"]
    bel = per_mp(rows, "bel")
    names = (
        "pv_death_claims",
        "pv_surrender_claims",
        "pv_maturity_claims",
        "pv_expenses",
        "initial_expense",
    )
    parts = {n: per_mp(rows, n) for n in names}
    premiums = per_mp(rows, "pv_premiums")
    for k, value in bel.items():
        expected = sum(parts[n][k] for n in names) - premiums[k]
        assert value == pytest.approx(expected, rel=1e-12, abs=1e-9)


def test_ifrs17_recognises_no_gain_at_initial_recognition(results):
    """The GMM identity: `LRC(0) = 0` when profitable, `= loss component` when onerous."""
    rows = results["ifrs17_gmm"]
    lrc0 = per_mp(rows, "lrc_at_issue")
    loss = per_mp(rows, "loss_component_initial")
    csm = per_mp(rows, "csm_initial")
    assert lrc0
    for k in lrc0:
        assert lrc0[k] == pytest.approx(loss[k], rel=1e-9, abs=1e-7)
        assert csm[k] * loss[k] == pytest.approx(0.0, abs=1e-9), "a contract is one or the other"
        assert csm[k] >= 0.0 and loss[k] >= 0.0


def test_ifrs17_csm_is_the_negative_of_fcf_plus_ra_when_profitable(results):
    rows = results["ifrs17_gmm"]
    csm = per_mp(rows, "csm_initial")
    bel = per_mp(rows, "bel")
    ra = per_mp(rows, "risk_adjustment")
    profitable = [k for k, v in csm.items() if v > 0]
    assert profitable, "the corpus is meant to contain profitable contracts"
    for k in profitable:
        assert csm[k] == pytest.approx(-(bel[k] + ra[k]), rel=1e-12, abs=1e-9)


def test_ifrs17_corpus_contains_both_onerous_and_profitable_contracts(results):
    onerous = [v for v in per_mp(results["ifrs17_gmm"], "is_onerous").values()]
    assert 0.0 in onerous and 1.0 in onerous


def test_ifrs17_csm_runs_off_to_zero(results):
    """The CSM must be fully released by the end of coverage — none may be left stranded."""
    rows = results["ifrs17_gmm"]
    for k, initial in per_mp(rows, "csm_initial").items():
        if initial <= 0:
            continue
        released = per_mp(rows, "pv_csm_release")[k]
        assert released > 0
        assert released == pytest.approx(initial, rel=0.05), (
            "the PV of the CSM released should be close to the CSM recognised at issue"
        )


def test_ifrs17_service_result_is_the_ra_and_csm_released(results):
    """On the central assumptions, revenue less expense is exactly what the balances released."""
    rows = results["ifrs17_gmm"]
    for k in per_mp(rows, "bel"):
        result = series_of(rows, k, "insurance_service_result")
        ra = series_of(rows, k, "ra_release")
        csm = series_of(rows, k, "csm_release")
        for got, a, c in zip(result, ra, csm):
            assert got == pytest.approx(a + c, rel=1e-12, abs=1e-9)


def test_ifrs17_uses_two_substage_levels(results):
    """`pv_csm_release` is a stage-2 value over a series seeded by a stage-2 value.

    Asserting it numerically rather than structurally: if levelling were wrong the CSM
    release would be identically zero (an unseeded balance) rather than merely different.
    """
    rows = results["ifrs17_gmm"]
    total = sum(per_mp(rows, "pv_csm_release").values())
    assert total > 0
    assert sum(per_mp(rows, "total_coverage_units").values()) > 0
