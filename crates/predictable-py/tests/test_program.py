"""`Program`, `check()` and the error hierarchy (`03-engine.md` §8, §8.2)."""

import pytest
from pathlib import PurePath

from predictable_engine import (
    CheckError,
    Plan,
    PredictableError,
    Program,
    check,
    format_pir,
)

from conftest import MODELPOINTS, TERM, run_pir

BROKEN = TERM.replace('expr = "survivors * q * sum_assured"', 'expr = "survivors * q * sum_asured"')


def test_a_clean_model_checks_with_no_errors(term_program):
    diagnostics = check(term_program)
    assert [d for d in diagnostics if d["severity"] == "error"] == []


def test_check_returns_the_json_diagnostics_of_ir_7():
    program = Program.from_sources({"term.pir": BROKEN})
    diagnostics = check(program)

    typo = next(d for d in diagnostics if d["code"] == "E0203")
    assert typo["severity"] == "error"
    assert "sum_asured" in typo["message"]
    # Every diagnostic carries a literal byte-range edit, not prose about a fix:
    # that is what lets an agent apply it and re-check without a human.
    assert typo["suggestions"][0]["edits"][0]["replacement"] == "sum_assured"
    assert typo["doc_url"]


def test_check_never_raises_for_a_broken_model():
    # Reporting the breakage *is* the job; a constructor that planned eagerly
    # would make a broken model uninspectable.
    program = Program.from_sources({"term.pir": BROKEN})
    assert len(check(program)) > 0


def test_planning_a_broken_model_raises_with_diagnostics_and_pretty():
    program = Program.from_sources({"term.pir": BROKEN})
    with pytest.raises(CheckError) as excinfo:
        program.plan()

    error = excinfo.value
    assert isinstance(error, PredictableError)
    assert error.diagnostics[0]["code"] == "E0203"
    # The traceback begins with a rendered diagnostic, not a stringified enum.
    assert "E0203" in error.pretty and "sum_asured" in error.pretty
    assert "E0203" in str(error)


def test_the_error_hierarchy_is_rooted_at_predictable_error():
    from predictable_engine import DataError, ParseError, TrapError

    for cls in (CheckError, DataError, ParseError, TrapError):
        assert issubclass(cls, PredictableError)
    assert issubclass(PredictableError, Exception)


def test_from_pir_reads_files_and_classifies_them(project):
    program = Program.from_pir([str(project / "term.pir"), str(project / "run.pir")])
    kinds = set(program.kinds.values())
    assert kinds == {"module", "run"}
    assert len(program.files) == 2


def test_from_pir_reads_a_directory_in_sorted_order(project):
    program = Program.from_pir(str(project))
    assert [PurePath(p).name for p in program.files] == ["run.pir", "term.pir"]


def test_model_digest_is_content_addressed_not_order_addressed():
    a = Program.from_sources({"term.pir": TERM, "b.pir": TERM.replace("term", "b")})
    b = Program.from_sources({"b.pir": TERM.replace("term", "b"), "term.pir": TERM})
    assert a.digest == b.digest

    # ... and whitespace is not content: the digest is over canonical text.
    spaced = Program.from_sources({"term.pir": TERM.replace("periods = 3", "periods  =  3")})
    assert spaced.digest == Program.from_sources({"term.pir": TERM}).digest


def test_the_run_file_travels_with_the_program(project):
    program = Program.from_pir(str(project))
    plan = program.plan()
    assert isinstance(plan, Plan)
    # `[run].modelpoints` is resolved relative to the program's base directory.
    result = plan.run()
    assert result.modelpoints == 4


def test_format_pir_is_the_canonical_form_and_is_idempotent():
    once = format_pir("term.pir", TERM)
    assert format_pir("term.pir", once) == once


def test_plan_exposes_the_planner_s_digests(term_program):
    plan = term_program.plan()
    assert len(plan.digest) == 64
    assert len(plan.order_digest) == 64
    assert plan.periods == 3
    # `emit = "outputs"` is the default, so only the three `Output` components.
    assert sorted(plan.components) == ["term.bel", "term.claims", "term.net_cashflow"]


def test_planning_twice_gives_the_same_order_digest(term_program):
    assert term_program.plan().order_digest == term_program.plan().order_digest


def test_emit_all_needs_full_retention_and_says_so(tmp_path):
    from predictable_engine import DataError

    # `emit = "all"` over ring-retained series cannot work: only the ring is kept,
    # so the whole series is not there to emit. The refusal names the fix.
    ring = Program.from_sources({"term.pir": TERM, "run.pir": run_pir(emit="all")})
    with pytest.raises(DataError, match='retain = "full"'):
        ring.plan().components

    full = Program.from_sources(
        {"term.pir": TERM, "run.pir": run_pir(emit="all", retain="full")}
    )
    assert "term.survivors" in full.plan().components
