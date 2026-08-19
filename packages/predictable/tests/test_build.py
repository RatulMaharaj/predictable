"""`predictable build [--check]` — task T21, `02-dsl.md` §8 and `01-ir.md` §11.1.

The end-to-end test is the one that matters and it is `test_the_fixture_builds_to_the_committed_bytes`:
DSL source → `.pir` → engine check → canonical output, compared byte for byte with the committed
fixture in `tests/fixtures/term_annual/build/`. Everything else in this file exists to say *why*
that comparison failed when it does.

Two fixtures are used throughout:

* `fixtures/term_annual/` — a complete, valid model with a committed build directory;
* `fixtures/broken/cycle.py` — two components that are individually fine and jointly a cycle, so
  the only thing that can catch it is the engine's checker, and the only useful place to report
  it is the Python.
"""

from __future__ import annotations

import hashlib
import importlib
import json
import shutil
import sys
from datetime import date
from pathlib import Path

import pytest

from predictable import registry_scope
from predictable.builder import (
    SPAN_MAP_FILE,
    build_product,
    check_build,
    engine_available,
    engine_check,
    import_models,
    remap_diagnostic,
    write_build,
)
from predictable.cli import build_command
from predictable.emit import EmitOptions, emit, format_value, module_file_names, table_digest
from predictable.spanmap import body_spans, component_blocks

HERE = Path(__file__).resolve().parent
FIXTURES = HERE / "fixtures"
TERM = FIXTURES / "term_annual"
COMMITTED = TERM / "build"

needs_engine = pytest.mark.skipif(
    not engine_available(),
    reason="predictable_engine (the T17 wheel) is not installed",
)


@pytest.fixture
def term_build():
    """The fixture model, freshly traced and emitted in an isolated registry."""
    with registry_scope():
        _fresh_import(TERM / "model.py")
        yield build_product(root=TERM)


@pytest.fixture
def broken_build():
    with registry_scope():
        _fresh_import(FIXTURES / "broken" / "cycle.py")
        yield build_product(root=FIXTURES / "broken")


def _fresh_import(path: Path) -> None:
    """Import a model file, discarding any cached copy — declarations must re-run."""
    sys.modules.pop(path.stem, None)
    import_models([path])
    importlib.invalidate_caches()


# --------------------------------------------------------------------------------------
# the end-to-end contract
# --------------------------------------------------------------------------------------


@needs_engine
def test_the_fixture_builds_to_the_committed_bytes(term_build):
    """DSL source → `.pir` → check → canonical output, against the committed fixture."""
    fresh = term_build.artefacts()
    committed = {p.name: p.read_text() for p in sorted(COMMITTED.iterdir()) if p.is_file()}
    assert set(fresh) == set(committed)
    for name in sorted(committed):
        assert fresh[name] == committed[name], f"{name} drifted from the committed build"


@needs_engine
def test_the_committed_build_passes_the_engines_own_checker():
    sources = {p.name: p.read_text() for p in COMMITTED.glob("*.pir")}
    diagnostics = engine_check(sources)
    assert [d for d in diagnostics if d["severity"] == "error"] == []


@needs_engine
def test_a_clean_build_reports_no_diagnostics_and_says_it_checked(term_build):
    assert term_build.checked
    assert term_build.ok
    assert term_build.diagnostics == ()


@needs_engine
def test_every_emitted_file_is_already_what_the_formatter_would_write():
    """The emitter aims at canonical form directly; `predictable fmt` is the judge (§4.1).

    `build_product` passes its output through the formatter before writing, so this test emits
    *raw* — otherwise it would be comparing the formatter with itself.
    """
    import predictable_engine

    from predictable import build

    with registry_scope():
        _fresh_import(TERM / "model.py")
        raw = emit(build(), EmitOptions(root=TERM, include_meta=False))
    for name, text in raw.files.items():
        assert predictable_engine.format_pir(name, text) == text, name


# --------------------------------------------------------------------------------------
# emitted structure (`01-ir.md` §4.1)
# --------------------------------------------------------------------------------------


def test_the_build_is_split_into_schema_modules_and_product(term_build):
    assert sorted(term_build.files) == ["model.pir", "product.pir", "schema.pir"]
    assert term_build.files["model.pir"].startswith('format = "pir/1"\nmodule = "model"\n')
    assert 'imports = ["schema"]' in term_build.files["model.pir"]
    assert 'modules = ["schema", "model"]' in term_build.files["product.pir"]


def test_component_keys_are_in_the_canonical_order(term_build):
    block = _block(term_build.files["model.pir"], "num_pols_if")
    keys = [line.split(" = ")[0] for line in block if " = " in line]
    assert keys == ["name", "kind", "dtype", "shape", "unit", "timing", "init", "expr", "doc"]


def test_a_scalar_shaped_component_carries_no_timing(term_build):
    block = _block(term_build.files["model.pir"], "pv_claims")
    assert 'shape = "PerMP"' in block
    assert not any(line.startswith("timing") for line in block)


def test_the_output_manifest_is_exactly_the_output_components(term_build):
    assert 'outputs = ["death_claims", "pv_claims"]' in term_build.files["product.pir"]


def _block(text: str, name: str) -> list[str]:
    lines = text.split("\n")
    start, end = component_blocks(text)[name]
    return [line for line in lines[start - 1 : end] if line and not line.startswith("[")]


# --------------------------------------------------------------------------------------
# table digests (`01-ir.md` §2.9)
# --------------------------------------------------------------------------------------


def test_the_table_digest_is_sha256_of_the_bytes(term_build):
    raw = (TERM / "tables" / "mortality.csv").read_bytes()
    expected = "sha256:" + hashlib.sha256(raw).hexdigest()
    assert term_build.table_digests["mortality"] == expected
    assert f'digest = "{expected}"' in term_build.files["schema.pir"]


def test_changing_one_byte_of_a_table_changes_its_digest(tmp_path):
    (tmp_path / "t.csv").write_text("age,qx\n40,0.001\n")
    first = table_digest("t.csv", tmp_path)
    (tmp_path / "t.csv").write_text("age,qx\n40,0.002\n")
    assert table_digest("t.csv", tmp_path) != first


def test_unresolvable_sources_have_no_digest_rather_than_a_wrong_one(tmp_path):
    assert table_digest("inline", tmp_path) is None
    assert table_digest("resource:supplied_by_host", tmp_path) is None
    assert table_digest("tables/absent.csv", tmp_path) is None


# --------------------------------------------------------------------------------------
# the span map (`01-ir.md` §11.1)
# --------------------------------------------------------------------------------------


def test_every_component_has_an_origin_span_pointing_at_its_def_line(term_build):
    source = (TERM / "model.py").read_text().split("\n")
    for component in term_build.span_map.components:
        origin = component.origin
        assert origin is not None, component.name
        assert f"def {component.name}(" in source[origin.line - 1]
        underlined = source[origin.line - 1][origin.col_start : origin.col_end]
        assert underlined == component.name


def test_the_span_map_locates_each_component_in_the_emitted_pir(term_build):
    lines = term_build.files["model.pir"].split("\n")
    for component in term_build.span_map.components:
        assert lines[component.pir_line - 1] == "[[component]]"
        assert f'name = "{component.name}"' in lines[component.pir_line]


def test_a_dependency_has_the_span_of_the_name_the_user_typed(term_build):
    qx = next(c for c in term_build.span_map.components if c.name == "qx")
    span = qx.refs["mortality_loading"]
    line = (TERM / "model.py").read_text().split("\n")[span.line - 1]
    assert line[span.col_start : span.col_end] == "mortality_loading"


def test_span_map_paths_are_relative_so_the_side_table_is_machine_independent(term_build):
    written = json.loads(term_build.artefacts()[SPAN_MAP_FILE])
    files = {c["origin_span"]["file"] for c in written["components"]}
    assert files == {"model.py"}


def test_body_spans_finds_names_in_the_body_and_in_the_signature():
    def example(alpha, beta):
        return alpha + beta

    spans = body_spans(example)
    assert set(spans) >= {"alpha", "beta"}
    assert spans["alpha"].line == spans["beta"].line


# --------------------------------------------------------------------------------------
# step 5: the engine's diagnostics, rendered against Python
# --------------------------------------------------------------------------------------


@needs_engine
def test_a_cycle_is_reported_against_two_python_bodies(broken_build):
    assert not broken_build.ok
    diagnostic = broken_build.errors[0]
    assert diagnostic.code == "E0201"
    assert len(diagnostic.spans) == 2
    for span in diagnostic.spans:
        assert span.file.endswith("cycle.py")
        assert span.text.strip().startswith("return")
    assert {s.start_pos.line for s in diagnostic.spans} == {23, 28}


@needs_engine
def test_the_underlined_token_is_the_dependency_that_closes_the_cycle(broken_build):
    diagnostic = broken_build.errors[0]
    underlined = {
        span.text[span.start_pos.column - 1 : span.start_pos.column - 1 + (span.end - span.start)]
        for span in diagnostic.spans
    }
    assert underlined == {"a", "b"}


@needs_engine
def test_the_pir_anchored_diagnostics_are_kept_alongside_the_python_ones(broken_build):
    assert broken_build.raw_diagnostics
    raw = broken_build.raw_diagnostics[0]
    assert raw["code"] == "E0201"
    assert all(span["file"].endswith(".pir") for span in raw["spans"])


@needs_engine
def test_a_refused_build_writes_nothing(tmp_path):
    code = build_command(
        [str(FIXTURES / "broken" / "cycle.py"), "--out", str(tmp_path / "build")],
        stdout=_Sink(),
        stderr=_Sink(),
    )
    assert code == 1
    assert not (tmp_path / "build").exists()


def test_a_diagnostic_with_no_known_span_still_renders_against_the_pir(term_build):
    index = term_build.span_map.index(term_build.files)
    raw = {
        "code": "E0999",
        "severity": "error",
        "message": "something in a file with no Python behind it",
        "spans": [
            {"file": "product.pir", "start": 0, "end": 4, "primary": True, "start_pos": {"line": 1, "column": 1}}
        ],
        "suggestions": [],
    }
    diagnostic = remap_diagnostic(raw, index)
    assert diagnostic.spans[0].file == "product.pir"
    assert any("generated `.pir`" in note for note in diagnostic.notes)


# --------------------------------------------------------------------------------------
# step 6: write or diff
# --------------------------------------------------------------------------------------


@needs_engine
def test_check_is_clean_against_the_committed_build(term_build):
    assert check_build(term_build, COMMITTED) == []


@needs_engine
def test_check_reports_the_first_differing_line(term_build, tmp_path):
    shutil.copytree(COMMITTED, tmp_path / "build")
    target = tmp_path / "build" / "model.pir"
    target.write_text(target.read_text().replace('unit = "years"', 'unit = "months"', 1))
    drift = check_build(term_build, tmp_path / "build")
    assert [d.file for d in drift] == ["model.pir"]
    assert "months" in drift[0].describe()


@needs_engine
def test_check_notices_a_file_that_is_no_longer_produced(term_build, tmp_path):
    shutil.copytree(COMMITTED, tmp_path / "build")
    (tmp_path / "build" / "stale.pir").write_text('format = "pir/1"\n')
    drift = check_build(term_build, tmp_path / "build")
    assert [(d.file, d.kind) for d in drift] == [("stale.pir", "removed")]


@needs_engine
def test_writing_removes_a_stale_pir_from_a_previous_build(term_build, tmp_path):
    stale = tmp_path / "stale.pir"
    stale.write_text('format = "pir/1"\n')
    write_build(term_build, tmp_path)
    assert not stale.exists()
    assert (tmp_path / "model.pir").exists()


# --------------------------------------------------------------------------------------
# the command
# --------------------------------------------------------------------------------------


class _Sink:
    def __init__(self) -> None:
        self.text = ""

    def write(self, s: str) -> int:
        self.text += s
        return len(s)

    def flush(self) -> None:  # pragma: no cover - argparse politeness
        pass


@needs_engine
def test_build_check_exits_zero_when_the_committed_build_is_current():
    out = _Sink()
    code = build_command(
        [str(TERM / "model.py"), "--check", "--root", str(TERM), "--out", str(COMMITTED)],
        stdout=out,
        stderr=_Sink(),
    )
    assert code == 0
    assert "up to date" in out.text


@needs_engine
def test_build_check_exits_one_on_drift(tmp_path):
    shutil.copytree(COMMITTED, tmp_path / "build")
    target = tmp_path / "build" / "schema.pir"
    target.write_text(target.read_text().replace("periods = 10", "periods = 11", 1))
    err = _Sink()
    code = build_command(
        [str(TERM / "model.py"), "--check", "--root", str(TERM), "--out", str(tmp_path / "build")],
        stdout=_Sink(),
        stderr=err,
    )
    assert code == 1
    assert "out of date" in err.text
    assert "schema.pir" in err.text


@needs_engine
def test_build_writes_a_complete_build_directory(tmp_path):
    out = tmp_path / "build"
    code = build_command(
        [str(TERM / "model.py"), "--root", str(TERM), "--out", str(out)],
        stdout=_Sink(),
        stderr=_Sink(),
    )
    assert code == 0
    assert sorted(p.name for p in out.iterdir()) == [
        "model.pir",
        "product.pir",
        "schema.pir",
        "spans.json",
    ]


@needs_engine
def test_json_output_carries_both_the_python_and_the_pir_diagnostics():
    out = _Sink()
    code = build_command(
        [str(FIXTURES / "broken" / "cycle.py"), "--json", "--out", "/tmp/predictable-build-json"],
        stdout=out,
        stderr=_Sink(),
    )
    assert code == 1
    report = json.loads(out.text)
    assert report["status"] == "failed"
    assert report["diagnostics"][0]["code"] == "E0201"
    assert report["diagnostics"][0]["spans"][0]["file"].endswith(".py")
    assert report["pir_diagnostics"][0]["spans"][0]["file"].endswith(".pir")


def test_an_unknown_path_is_a_usage_error():
    err = _Sink()
    assert build_command(["does/not/exist.py"], stdout=_Sink(), stderr=err) == 2
    assert "no such path" in err.text


def test_the_module_entry_point_rejects_an_unknown_command():
    from predictable.cli import main

    assert main(["fmt"]) == 2


# --------------------------------------------------------------------------------------
# rendering units
# --------------------------------------------------------------------------------------


def test_floats_are_written_in_shortest_round_trip_form():
    assert format_value(1.0) == "1.0"
    assert format_value(0.1 + 0.2) == "0.30000000000000004"
    assert format_value(1e300) == "1e300"


def test_dates_strings_and_bools_are_written_as_toml():
    assert format_value(date(2026, 6, 30)) == "2026-06-30"
    assert format_value('a "quoted" path\\here') == '"a \\"quoted\\" path\\\\here"'
    assert format_value(True) == "true"


def test_an_array_of_inline_tables_breaks_one_per_line():
    one = format_value([{"name": "age"}])
    many = format_value([{"name": "age"}, {"name": "gender"}])
    assert one == '[{ name = "age" }]'
    assert many == '[\n  { name = "age" },\n  { name = "gender" },\n]'


def test_a_flat_array_stays_on_one_line():
    assert format_value(["a", "b", "c"]) == '["a", "b", "c"]'


def test_module_names_are_leaves_until_two_modules_share_one():
    assert module_file_names(["models.term.decrements"]) == {"models.term.decrements": "decrements"}
    clash = module_file_names(["a.model", "b.model"])
    assert clash == {"a.model": "a_model", "b.model": "b_model"}


# --------------------------------------------------------------------------------------
# degrading without the engine
# --------------------------------------------------------------------------------------


def test_without_the_engine_the_build_still_emits_and_says_it_did_not_check(monkeypatch):
    """A missing wheel must not silently look like a clean check (`02-dsl.md` §8 step 5)."""
    from predictable import builder

    monkeypatch.setattr(builder, "_engine", lambda: None)
    with registry_scope():
        _fresh_import(TERM / "model.py")
        result = builder.build_product(root=TERM)
    assert result.checked is False
    assert result.meta_in_pir is False
    assert sorted(result.files) == ["model.pir", "product.pir", "schema.pir"]


def test_the_build_api_is_exported_from_the_package():
    import predictable

    for name in ("build_product", "write_build", "check_build", "emit", "build_span_map"):
        assert name in predictable.__all__
        assert hasattr(predictable, name)
