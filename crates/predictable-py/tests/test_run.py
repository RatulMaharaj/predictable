"""`plan.run(...)`: modelpoints in, Arrow out (`03-engine.md` §8.1, §8.2)."""

import math

import pyarrow as pa
import pyarrow.csv
import pytest

from predictable_engine import DataError, Program, TrapError

from conftest import MODELPOINTS, TERM, TRAPPING, run_pir


def expected_bel(sum_assured, premium, q, periods=3):
    """The term model, by hand — the oracle these tests check the engine against."""
    survivors = 1.0
    total = 0.0
    for _ in range(periods + 1):
        claims = survivors * q * sum_assured
        total += claims - survivors * premium
        survivors = survivors * (1 - q)
    return total


def bels(result):
    table = result.to_arrow()
    rows = table.to_pylist()
    return {
        r["mp_key"]: r["value"] for r in rows if r["component"] == "term.bel"
    }


def test_a_run_produces_the_numbers_the_model_says_it_should(project):
    result = Program.from_pir(str(project)).plan().run()

    assert result.outcome == "completed"
    assert result.exit_code == 0
    assert result.modelpoints == 4

    got = bels(result)
    assert got["POL1"] == pytest.approx(expected_bel(100000, 900, 0.01))
    assert got["POL4"] == pytest.approx(expected_bel(75000, 600, 0.03))


def test_the_long_format_is_the_schema_of_verify_2(project):
    table = Program.from_pir(str(project)).plan().run().to_arrow()
    assert table.column_names == [
        "mp_key",
        "mp_row",
        "component",
        "stage",
        "t",
        "value",
        "value_i",
        "value_b",
        "value_s",
    ]
    # A `Series` carries `t = 0..=T`; a `PerMP` carries `t = -1`.
    series = table.filter(pa.compute.equal(table["component"], "term.claims"))
    assert sorted(set(series["t"].to_pylist())) == [0, 1, 2, 3]
    per_mp = table.filter(pa.compute.equal(table["component"], "term.bel"))
    assert set(per_mp["t"].to_pylist()) == {-1}
    # `stage` is the IR's, never re-derived: `bel` is a stage-2 reduction.
    assert set(per_mp["stage"].to_pylist()) == {2}


def test_results_are_in_chunk_then_offset_order(project):
    result = Program.from_pir(str(project)).plan().run(chunk_size=1)
    keys = result.to_arrow()["mp_key"].to_pylist()
    # First appearance order is file order, whatever the chunking was.
    seen = list(dict.fromkeys(keys))
    assert seen == ["POL1", "POL2", "POL3", "POL4"]


def test_chunking_does_not_change_a_single_number(project):
    plan = Program.from_pir(str(project)).plan()
    one = plan.run(chunk_size=1).to_arrow().to_pylist()
    many = plan.run(chunk_size=1024).to_arrow().to_pylist()
    assert one == many


def test_threads_do_not_change_a_single_number(project):
    plan = Program.from_pir(str(project)).plan()
    serial = plan.run(chunk_size=1, threads=1).to_arrow().to_pylist()
    parallel = plan.run(chunk_size=1, threads=4).to_arrow().to_pylist()
    assert serial == parallel


def test_a_plan_is_reusable(project):
    plan = Program.from_pir(str(project)).plan()
    assert plan.run().rows == plan.run().rows


def test_assumptions_override_the_declared_set(tmp_path):
    src = TERM.replace(
        '[[component]]\nname = "survivors"',
        """[[component]]
name = "shock"
kind = "Input.Assumption"
dtype = "f64"
shape = "Scalar"
unit = "none"

[[component]]
name = "survivors\"""",
    ).replace('expr = "survivors * q * sum_assured"', 'expr = "survivors * q * sum_assured * shock"')
    (tmp_path / "term.pir").write_text(src)
    (tmp_path / "modelpoints.csv").write_text(MODELPOINTS)
    plan = Program.from_pir(str(tmp_path / "term.pir")).plan()

    base = bels(plan.run(str(tmp_path / "modelpoints.csv"), assumptions={"shock": 1.0}))
    shocked = bels(plan.run(str(tmp_path / "modelpoints.csv"), assumptions={"shock": 2.0}))
    assert shocked["POL1"] > base["POL1"]


def test_modelpoints_come_from_a_pyarrow_table_zero_copy(project):
    plan = Program.from_pir(str(project)).plan()
    table = pa.table(
        {
            "policy_number": ["POL1", "POL2"],
            "product_code": ["TERM_UK", "TERM_UK"],
            "sum_assured": [100000.0, 250000.0],
            "premium": [900.0, 1500.0],
            "q": [0.01, 0.02],
        }
    )
    got = bels(plan.run(table))
    assert got["POL1"] == pytest.approx(expected_bel(100000, 900, 0.01))
    assert set(got) == {"POL1", "POL2"}


def test_arrow_in_equals_csv_in(project):
    plan = Program.from_pir(str(project)).plan()
    from_csv = plan.run().to_arrow().to_pylist()
    reader = pa.csv.read_csv(project / "modelpoints.csv")
    from_arrow = plan.run(reader).to_arrow().to_pylist()
    assert from_csv == from_arrow


def test_results_export_the_arrow_c_stream_without_pyarrow(project):
    result = Program.from_pir(str(project)).plan().run()
    capsule = result.__arrow_c_stream__()
    # The capsule is the standard C Data Interface hand-off; pyarrow is only one
    # possible consumer of it.
    assert "arrow_array_stream" in repr(capsule)
    assert pa.table(result).num_rows == result.rows


def test_the_results_schema_cannot_be_renegotiated(project):
    result = Program.from_pir(str(project)).plan().run()
    with pytest.raises(DataError, match="fixed"):
        result.__arrow_c_stream__(pa.schema([pa.field("x", pa.int64())]))


def test_to_pandas_round_trips(project):
    pytest.importorskip("pandas")
    df = Program.from_pir(str(project)).plan().run().to_pandas()
    assert list(df.columns)[:3] == ["mp_key", "mp_row", "component"]
    assert len(df) > 0


def test_a_missing_required_column_fails_before_row_one(project, tmp_path):
    bad = tmp_path / "bad.csv"
    bad.write_text("policy_number,product_code,sum_assured,premium\nPOL1,TERM_UK,1,1\n")
    plan = Program.from_pir(str(project)).plan()
    with pytest.raises(DataError) as excinfo:
        plan.run(str(bad))
    assert "q" in str(excinfo.value)


def test_an_unknown_modelpoint_format_is_refused(project):
    plan = Program.from_pir(str(project)).plan()
    with pytest.raises(DataError, match="not a modelpoint format"):
        plan.run(str(project / "term.pir"))


def test_a_trap_under_abort_raises_with_the_e0902_envelope(tmp_path):
    (tmp_path / "trap.pir").write_text(TRAPPING)
    (tmp_path / "mp.csv").write_text("policy_number,numerator,exposure\nP1,10,2\nP2,10,0\n")
    plan = Program.from_pir(str(tmp_path / "trap.pir")).plan()

    with pytest.raises(TrapError) as excinfo:
        plan.run(str(tmp_path / "mp.csv"))

    envelope = excinfo.value.diagnostics[0]
    assert envelope["code"] == "E0902"
    assert envelope["mp_key"] == "P2"
    assert envelope["component"].endswith("ratio")
    # The operands are the real ones, captured on replay — not a guess.
    assert dict(envelope["operands"])["rhs"] == 0.0


def test_a_trap_under_continue_drops_the_modelpoint_and_says_so(tmp_path):
    (tmp_path / "trap.pir").write_text(TRAPPING)
    (tmp_path / "run.pir").write_text(
        run_pir(on_trap="continue").replace('product = "term"', 'product = "trap"')
    )
    (tmp_path / "mp.csv").write_text("policy_number,numerator,exposure\nP1,10,2\nP2,10,0\n")
    plan = Program.from_pir([str(tmp_path / "trap.pir"), str(tmp_path / "run.pir")]).plan()

    result = plan.run(str(tmp_path / "mp.csv"))
    assert result.outcome == "completed_with_traps"
    assert result.exit_code == 1
    assert result.modelpoints_trapped == 1
    # Q7: the trapping modelpoint is *gone*, not present with nulls.
    assert set(result.to_arrow()["mp_key"].to_pylist()) == {"P1"}
    assert result.traps[0]["code"] == "E0902"


def test_progress_is_called_between_chunks_with_counts_only(project):
    seen = []
    plan = Program.from_pir(str(project)).plan()
    result = plan.run(chunk_size=1, progress=lambda done, total: seen.append((done, total)))

    assert seen[-1] == (4, 4)
    assert [d for d, _ in seen] == sorted(d for d, _ in seen)
    # A callback cannot influence results.
    assert result.outcome == "completed"


def test_a_progress_callback_that_raises_cancels_the_run(project):
    def stop(done, total):
        raise RuntimeError("user pressed stop")

    plan = Program.from_pir(str(project)).plan()
    with pytest.raises(KeyboardInterrupt):
        plan.run(chunk_size=1, progress=stop)


def test_aggregations_are_folded_in_chunk_index_order(tmp_path):
    (tmp_path / "term.pir").write_text(TERM)
    (tmp_path / "modelpoints.csv").write_text(MODELPOINTS)
    (tmp_path / "run.pir").write_text(
        run_pir()
        + """
[[aggregation]]
name = "bel_by_product"
group_by = ["product_code"]
measure = "bel"
op = "sum"
"""
    )
    plan = Program.from_pir(str(tmp_path)).plan()
    one = plan.run(chunk_size=1)
    many = plan.run(chunk_size=1024)

    keys = {row["group_key"]: row["value"] for row in one.aggregates}
    assert set(keys) == {"product_code=TERM_UK", "product_code=TERM_IE"}
    assert keys["product_code=TERM_UK"] == pytest.approx(
        expected_bel(100000, 900, 0.01) + expected_bel(250000, 1500, 0.02)
    )
    # The fold is over chunk index, not completion order, so C makes no difference.
    assert one.aggregates == many.aggregates


def test_the_run_reports_what_it_emitted(project):
    result = Program.from_pir(str(project)).plan().run()
    assert sorted(result.components) == ["term.bel", "term.claims", "term.net_cashflow"]
    assert result.trap_count == 0
    assert math.isfinite(result.rows)
    assert "outcome=completed" in repr(result)
