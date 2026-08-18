"""`predictable build [--check]` — steps 4 to 6 of `02-dsl.md` §8.

    4. **Emit.** Render each component to canonical `.pir`, compute table digests, emit the
       schema blocks.
    5. **Full check.** Shell out to the engine's checker — *in process*, through the PyO3 wheel
       — over the emitted text, and map every diagnostic back onto the Python the user wrote.
    6. **Write or diff.** ``--check`` compares to the committed ``build/`` and exits non-zero on
       drift.

The load-bearing part is step 5's second half. The engine's checker knows nothing about Python;
it reports byte ranges in `model.pir`. Every diagnostic that comes back is re-anchored through
:mod:`predictable.spanmap` and re-rendered against the `.py` file, so an `E0201` cycle is shown
as two Python function bodies. The original `.pir` span is never thrown away — it stays in the
JSON as ``pir_span`` — because the emitted text is a real artefact a user can open.

The checker is the *installed engine's*, never a Python re-implementation: this module imports
``predictable_engine`` (task T17) and calls ``check`` on an in-memory program. If the wheel is
not installed the build still emits, and says so, rather than pretending the model is checked.
"""

from __future__ import annotations

import json
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .declarations import BuiltModel, registry
from .declarations import build as build_model
from .diagnostics import Diagnostic, LineCol, Span, Suggestion, render
from .emit import EmitOptions, Emitted, emit
from .spanmap import SpanIndex, SpanMap, build_span_map

__all__ = [
    "SPAN_MAP_FILE",
    "BuildResult",
    "build_product",
    "check_build",
    "engine_available",
    "engine_check",
    "write_build",
]

SPAN_MAP_FILE = "spans.json"
"""The `ExprId → origin_span` side table `01-ir.md` §11.1 requires alongside the modules."""


# --------------------------------------------------------------------------------------
# the engine, in process
# --------------------------------------------------------------------------------------


def _engine() -> Any | None:
    try:
        import predictable_engine  # type: ignore

        return predictable_engine
    except ImportError:  # pragma: no cover - depends on the environment
        return None


def engine_available() -> bool:
    """True when the T17 wheel is importable, i.e. when step 5 can actually run."""
    return _engine() is not None


_META_PROBE = """format = "pir/1"
module = "probe"

[[component]]
name = "x"
kind = "Output"
dtype = "f64"
shape = "Scalar"
unit = "money"
expr = "1.0"

[component.meta]
authored_by = "dsl"
origin_span = { file = "p.py", line = 1, col_start = 0, col_end = 1 }
"""


def _meta_supported(engine: Any) -> bool:
    """Does the installed checker accept `[component.meta]` (`01-ir.md` §11.1)?

    `meta` is normative and optional, and a checker that has not implemented it rejects the whole
    section. Emitting text the installed checker refuses would make every build unbuildable, so
    the emitter asks first and falls back to carrying provenance in ``spans.json`` alone. The
    probe is one in-memory module and costs microseconds.
    """
    try:
        program = engine.Program.from_sources({"probe.pir": _META_PROBE})
        diagnostics = engine.check(program)
    except Exception:  # pragma: no cover - a broken wheel is reported elsewhere
        return False
    return not any(d.get("code") in {"E0044", "E0045"} for d in diagnostics)


def engine_check(files: dict[str, str]) -> list[dict[str, Any]]:
    """Run the engine's checker over in-memory `.pir` sources. Raises if the wheel is missing."""
    engine = _engine()
    if engine is None:
        raise RuntimeError(
            "predictable_engine is not installed; `predictable build` cannot run the engine "
            "checker. Install the engine wheel (maturin develop -m crates/predictable-py)."
        )
    try:
        program = engine.Program.from_sources(dict(files))
    except Exception as exc:  # a parse failure of our own emitted text
        return [
            {
                "code": "E0001",
                "severity": "error",
                "message": str(exc),
                "spans": [],
                "suggestions": [],
                "doc_url": "https://predictable.dev/llm/diagnostics/#E0001",
            }
        ]
    return list(engine.check(program))


# --------------------------------------------------------------------------------------
# diagnostics, re-anchored on Python
# --------------------------------------------------------------------------------------


def remap_diagnostic(raw: dict[str, Any], index: SpanIndex) -> Diagnostic:
    """One engine diagnostic, re-rendered against Python source (`02-dsl.md` §8)."""
    spans: list[Span] = []
    for raw_span in raw.get("spans", ()):
        line = (raw_span.get("start_pos") or {}).get("line", 0)
        py_span, covered = index.lookup(
            raw_span.get("file", ""), line, raw_span.get("start", 0), raw_span.get("end", 0)
        )
        if py_span is None:
            spans.append(_as_pir_span(raw_span))
            continue
        spans.append(
            py_span.to_diagnostic_span(
                label=raw_span.get("label"), primary=raw_span.get("primary", True)
            )
        )
        del covered
    suggestions = tuple(
        Suggestion(message=s.get("message", ""), edits=(), applicability=s.get("applicability", "maybe_incorrect"))
        for s in raw.get("suggestions", ())
    )
    notes = tuple(raw.get("notes", ()))
    if any(s.file.endswith(".pir") for s in spans):
        notes = notes + (
            (
                "reported against the generated `.pir`: no Python span was recorded for this "
                "location. The emitted file is in the build directory."
            ),
        )
    return Diagnostic(
        code=raw.get("code", "E0000"),
        message=raw.get("message", ""),
        spans=tuple(spans),
        suggestions=suggestions,
        notes=notes,
        severity=raw.get("severity", "error"),
    )


def _as_pir_span(raw_span: dict[str, Any]) -> Span:
    pos = raw_span.get("start_pos") or {}
    return Span(
        file=raw_span.get("file", "<pir>"),
        start=raw_span.get("start", 0),
        end=raw_span.get("end", 0),
        primary=raw_span.get("primary", True),
        label=raw_span.get("label"),
        start_pos=LineCol(pos.get("line", 0), pos.get("column", 1)),
        text="",
    )


# --------------------------------------------------------------------------------------
# the build
# --------------------------------------------------------------------------------------


@dataclass(frozen=True)
class BuildResult:
    """Everything one `predictable build` produced, whether or not it was written."""

    files: dict[str, str]
    span_map: SpanMap
    table_digests: dict[str, str | None]
    #: Engine diagnostics as returned, `.pir`-anchored.
    raw_diagnostics: tuple[dict[str, Any], ...] = ()
    #: The same diagnostics re-anchored on Python, plus the DSL's own build lints.
    diagnostics: tuple[Diagnostic, ...] = ()
    checked: bool = True
    meta_in_pir: bool = True
    #: Paths in ``spans.json`` are written relative to this directory.
    root: Path | None = None

    @property
    def errors(self) -> tuple[Diagnostic, ...]:
        return tuple(d for d in self.diagnostics if d.severity == "error")

    @property
    def ok(self) -> bool:
        return not self.errors

    def artefacts(self) -> dict[str, str]:
        """The files as they are written to disk, ``spans.json`` included."""
        out = dict(self.files)
        out[SPAN_MAP_FILE] = json.dumps(self.span_map.to_json(self.root), indent=2) + "\n"
        return out

    def render(self) -> str:
        return "\n\n".join(render(d) for d in self.diagnostics)


def build_product(
    product: str | None = None,
    *,
    root: Path | str | None = None,
    reg: Any | None = None,
    check: bool = True,
    model: BuiltModel | None = None,
) -> BuildResult:
    """Trace, emit, digest, span-map and check. The whole of §8 in one call."""
    active = reg if reg is not None else registry()
    built = model if model is not None else build_model(product, reg=active)

    engine = _engine()
    include_meta = _meta_supported(engine) if engine is not None else False
    options = EmitOptions(root=Path(root) if root is not None else Path.cwd(), include_meta=include_meta)
    emitted: Emitted = emit(built, options)
    if engine is not None:
        # The formatter is the authority on canonical form (`01-ir.md` §4.1). The emitter aims to
        # land on it directly — `test_build.py` asserts it always does — but the *written* text is
        # the formatter's, so a build directory can never contain text `predictable fmt` would
        # rewrite and `--check` can never report drift the author cannot reproduce.
        emitted = _canonicalise(engine, emitted)

    span_map = build_span_map(built.components, emitted.files, emitted.file_modules, reg=active)
    index = span_map.index(emitted.files)

    diagnostics: list[Diagnostic] = list(built.warnings)
    raw: list[dict[str, Any]] = []
    ran_check = False
    if check and engine is not None:
        raw = engine_check(emitted.files)
        diagnostics += [remap_diagnostic(d, index) for d in raw]
        ran_check = True

    return BuildResult(
        files=emitted.files,
        span_map=span_map,
        table_digests=emitted.table_digests,
        raw_diagnostics=tuple(raw),
        diagnostics=tuple(diagnostics),
        checked=ran_check,
        meta_in_pir=include_meta,
        root=options.root,
    )


def _canonicalise(engine: Any, emitted: Emitted) -> Emitted:
    files = {name: engine.format_pir(name, text) for name, text in emitted.files.items()}
    return Emitted(
        files=files,
        table_digests=emitted.table_digests,
        module_names=emitted.module_names,
        file_modules=emitted.file_modules,
    )


def write_build(result: BuildResult, out_dir: Path | str) -> list[Path]:
    """Write the build directory. Stale `.pir` files from a previous build are removed."""
    out = Path(out_dir)
    out.mkdir(parents=True, exist_ok=True)
    artefacts = result.artefacts()
    for existing in sorted(out.glob("*.pir")):
        if existing.name not in artefacts:
            existing.unlink()
    written = []
    for name in sorted(artefacts):
        path = out / name
        path.write_text(artefacts[name], encoding="utf-8", newline="\n")
        written.append(path)
    return written


@dataclass(frozen=True)
class Drift:
    """One difference between a fresh build and the committed one (`--check`)."""

    file: str
    kind: str  # "added" | "removed" | "changed"
    committed: str | None = None
    fresh: str | None = None

    def describe(self) -> str:
        if self.kind == "added":
            return f"{self.file}: not in the committed build"
        if self.kind == "removed":
            return f"{self.file}: committed but no longer produced"
        return f"{self.file}: {_first_difference(self.committed or '', self.fresh or '')}"


def _first_difference(a: str, b: str) -> str:
    a_lines, b_lines = a.split("\n"), b.split("\n")
    for i, (x, y) in enumerate(zip(a_lines, b_lines), start=1):
        if x != y:
            return f"line {i}: committed {x!r}, built {y!r}"
    return f"line {min(len(a_lines), len(b_lines)) + 1}: one file is longer than the other"


def check_build(result: BuildResult, out_dir: Path | str) -> list[Drift]:
    """`--check`: what would change if this build were written. Empty means up to date.

    ``spans.json`` is compared too: a moved function body changes provenance even when it does
    not change a single number, and a provenance side table that silently rots is worse than
    none at all.
    """
    out = Path(out_dir)
    artefacts = result.artefacts()
    committed = {
        p.name: p.read_text(encoding="utf-8")
        for p in sorted(out.glob("*"))
        if p.is_file() and (p.suffix == ".pir" or p.name == SPAN_MAP_FILE)
    }
    drift: list[Drift] = []
    for name in sorted(set(artefacts) | set(committed)):
        fresh = artefacts.get(name)
        old = committed.get(name)
        if old is None:
            drift.append(Drift(name, "added", None, fresh))
        elif fresh is None:
            drift.append(Drift(name, "removed", old, None))
        elif old != fresh:
            drift.append(Drift(name, "changed", old, fresh))
    return drift


# --------------------------------------------------------------------------------------
# importing the user's model
# --------------------------------------------------------------------------------------


def import_models(paths: Sequence[str | Path]) -> list[str]:
    """Import each `.py` path (or every `.py` in a directory) so its decorators run.

    Step 1 of §8, "collect": a model is declared by importing it, and `declaration_index` — and
    therefore evaluation order — is the order in which those imports happen.
    """
    import importlib.util
    import sys

    files: list[Path] = []
    for raw in paths:
        path = Path(raw)
        if path.is_dir():
            files += sorted(p for p in path.glob("*.py") if not p.name.startswith("_"))
        else:
            files.append(path)

    imported = []
    for path in files:
        name = path.stem
        spec = importlib.util.spec_from_file_location(name, path)
        if spec is None or spec.loader is None:  # pragma: no cover
            raise RuntimeError(f"cannot import {path}")
        module = importlib.util.module_from_spec(spec)
        parent = str(path.resolve().parent)
        if parent not in sys.path:
            sys.path.insert(0, parent)
        sys.modules[name] = module
        spec.loader.exec_module(module)
        imported.append(name)
    return imported


def default_out_dir(paths: Sequence[str | Path]) -> Path:
    first = Path(paths[0]) if paths else Path.cwd()
    base = first if first.is_dir() else first.parent
    return base / "build"
