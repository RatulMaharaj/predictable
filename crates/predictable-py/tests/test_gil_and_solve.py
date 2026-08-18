"""The GIL contract (§8.2) and the `[[solve]]` outer loop (IR §8.4.4)."""

import threading
import time

import pytest

from predictable_engine import Program

from conftest import TERM, run_pir


def big_modelpoints(n):
    rows = ["policy_number,product_code,sum_assured,premium,q"]
    for i in range(n):
        rows.append(f"POL{i:05d},TERM_UK,{100000 + i},{900 + i},0.01")
    return "\n".join(rows) + "\n"


def test_the_gil_is_released_for_the_whole_projection(tmp_path):
    (tmp_path / "term.pir").write_text(TERM)
    (tmp_path / "mp.csv").write_text(big_modelpoints(40000))
    plan = Program.from_pir(str(tmp_path / "term.pir")).plan()

    ticks = [0]
    stop = threading.Event()

    def spin():
        while not stop.is_set():
            ticks[0] += 1
            time.sleep(0.001)

    watcher = threading.Thread(target=spin)
    watcher.start()
    try:
        before = ticks[0]
        result = plan.run(str(tmp_path / "mp.csv"), chunk_size=256)
        during = ticks[0] - before
    finally:
        stop.set()
        watcher.join()

    assert result.modelpoints == 40000
    # If `run()` held the GIL the watcher could not have run at all. It is a
    # timing assertion, but a very loose one: any progress at all disproves a lock.
    assert during > 0


def test_a_per_mp_solve_reaches_its_target(tmp_path):
    (tmp_path / "term.pir").write_text(TERM)
    (tmp_path / "mp.csv").write_text(
        "policy_number,product_code,sum_assured,premium,q\n"
        "POL1,TERM_UK,100000,900,0.01\n"
        "POL2,TERM_UK,250000,1500,0.02\n"
    )
    (tmp_path / "run.pir").write_text(
        run_pir()
        + """
[[solve]]
name = "breakeven_premium"
target = "bel"
to = 0.0
vary = "premium"
scope = "per_mp"
bracket = [0.0, 100000.0]
tolerance = 1e-9
"""
    )
    plan = Program.from_pir([str(tmp_path / "term.pir"), str(tmp_path / "run.pir")]).plan()
    result = plan.run(str(tmp_path / "mp.csv"))

    outcome = result.solves[0]
    assert outcome["name"] == "breakeven_premium"
    assert outcome["converged"] == 2
    assert outcome["not_converged"] == 0

    # Q8: the solved value is an ordinary `PerMP` component of the result set, so a
    # downstream join never has to special-case a solve...
    rows = result.to_arrow().to_pylist()
    solved = {r["mp_key"]: r["value"] for r in rows if r["component"] == "breakeven_premium"}
    assert set(solved) == {"POL1", "POL2"}

    # ... and the final projection is *the solved projection*: `bel` is at target.
    bel = {r["mp_key"]: r["value"] for r in rows if r["component"] == "term.bel"}
    assert bel["POL1"] == pytest.approx(0.0, abs=1e-6)
    assert bel["POL2"] == pytest.approx(0.0, abs=1e-6)
