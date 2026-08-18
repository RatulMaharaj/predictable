"""The Python ↔ `.pir` span map (`01-ir.md` §11.1, `02-dsl.md` §8).

> Span mapping is the piece that must not be cut … an engine-level `E0201` cycle error shows two
> Python function bodies, not two TOML lines.

Two directions are needed and both are built here.

*Forward*, into the IR: each component's `meta.origin_span` is the `def` line of the Python
function it was traced from, and a side table keyed by component name carries the span of every
*name occurrence* inside the body — which is the `ExprId → origin_span` side table §11.1 asks for,
keyed by the thing a diagnostic can actually name. It is derived from the function's own AST, so a
span points at the identifier the user typed, columns included, not at the whole line.

*Backward*, out of the IR: :class:`SpanIndex` answers "this byte range of `model.pir`, on this
line — whose Python is that?". The engine's checker reports against the emitted text; the answer
is what lets `predictable build` re-render its diagnostics against the `.py` file instead.
"""

from __future__ import annotations

import ast
import inspect
import linecache
import textwrap
from collections.abc import Callable, Iterable, Sequence
from dataclasses import dataclass, field
from typing import Any

from .declarations import BuiltComponent
from .diagnostics import LineCol, Span

__all__ = [
    "ComponentSpans",
    "PySpan",
    "SpanIndex",
    "SpanMap",
    "body_spans",
    "build_span_map",
    "relative_file",
]


def relative_file(file: str, base: Any = None) -> str:
    """``file`` relative to ``base`` when it is underneath it, else unchanged.

    A committed build directory is compared byte for byte by ``--check``, so an absolute path
    from whoever ran the build last would be permanent, meaningless drift.
    """
    if base is None:
        return file
    from pathlib import Path as _Path

    try:
        return _Path(file).resolve().relative_to(_Path(base).resolve()).as_posix()
    except (ValueError, OSError):  # pragma: no cover - different drive / unreadable
        return file


@dataclass(frozen=True)
class PySpan:
    """A span in Python source, in the shape `meta.origin_span` uses."""

    file: str
    line: int
    col_start: int
    col_end: int

    def to_json(self, base: Any = None) -> dict[str, Any]:
        """``base`` makes ``file`` relative, so a committed ``spans.json`` is machine-independent."""
        return {
            "file": relative_file(self.file, base),
            "line": self.line,
            "col_start": self.col_start,
            "col_end": self.col_end,
        }

    def to_diagnostic_span(self, label: str | None = None, primary: bool = True) -> Span:
        """The same span as a :class:`predictable.diagnostics.Span`, ready to render."""
        text = linecache.getline(self.file, self.line).rstrip("\n")
        start = _byte_offset(self.file, self.line, self.col_start)
        width = max(self.col_end - self.col_start, 1)
        return Span(
            file=self.file,
            start=start,
            end=start + width,
            primary=primary,
            label=label,
            start_pos=LineCol(self.line, self.col_start + 1),
            text=text,
        )


def _byte_offset(file: str, line: int, col: int) -> int:
    total = 0
    for i in range(1, line):
        text = linecache.getline(file, i)
        if not text:
            break
        total += len(text.encode())
    return total + col


# --------------------------------------------------------------------------------------
# forward: Python source → spans
# --------------------------------------------------------------------------------------


def body_spans(func: Callable[..., Any]) -> dict[str, PySpan]:
    """Every name occurrence in a component body, as ``name -> span of its first use``.

    ``qx[t - 1]``, ``qx(...)`` and a bare ``qx`` all yield the span of ``qx`` itself, because the
    identifier is what a resolution, cycle or unit diagnostic names.
    """
    try:
        source = inspect.getsource(func)
    except (OSError, TypeError):  # pragma: no cover - eval'd or C functions
        return {}
    file = inspect.getsourcefile(func) or func.__code__.co_filename
    first = func.__code__.co_firstlineno
    try:
        tree = ast.parse(textwrap.dedent(source))
    except SyntaxError:  # pragma: no cover - the function compiled, so this cannot happen
        return {}
    indent = _indent_of(source)

    out: dict[str, PySpan] = {}
    for node in ast.walk(tree):
        if not isinstance(node, ast.Name):
            continue
        span = PySpan(
            file=file,
            line=first + node.lineno - 1,
            col_start=node.col_offset + indent,
            col_end=(node.end_col_offset or node.col_offset + len(node.id)) + indent,
        )
        out.setdefault(node.id, span)
    return out


def _indent_of(source: str) -> int:
    for line in source.splitlines():
        if line.strip():
            return len(line) - len(line.lstrip())
    return 0  # pragma: no cover


def _def_span(func: Callable[..., Any]) -> PySpan | None:
    code = getattr(func, "__code__", None)
    if code is None:  # pragma: no cover
        return None
    file = inspect.getsourcefile(func) or code.co_filename
    line = code.co_firstlineno
    text = linecache.getline(file, line)
    # Underline the name in `def name(...)` when the decorator line is what co_firstlineno points
    # at; fall back to the whole trimmed line.
    while text and text.lstrip().startswith("@"):
        line += 1
        text = linecache.getline(file, line)
    stripped = text.rstrip("\n")
    col = stripped.find(func.__name__)
    if col < 0:
        col, end = len(stripped) - len(stripped.lstrip()), len(stripped)
    else:
        end = col + len(func.__name__)
    return PySpan(file=file, line=line, col_start=col, col_end=end)


@dataclass(frozen=True)
class ComponentSpans:
    """Everything known about where one component came from and where it landed."""

    name: str
    pir_file: str
    #: 1-based line of this component's `[[component]]` header in the emitted `.pir`.
    pir_line: int
    #: Last line of the component's block, inclusive.
    pir_end_line: int
    origin: PySpan | None
    #: Name occurrences inside the Python body, the `ExprId → origin_span` side table of §11.1.
    refs: dict[str, PySpan] = field(default_factory=dict)

    def to_json(self, base: Any = None) -> dict[str, Any]:
        return {
            "name": self.name,
            "pir_file": self.pir_file,
            "pir_line": self.pir_line,
            "pir_end_line": self.pir_end_line,
            "origin_span": self.origin.to_json(base) if self.origin else None,
            "refs": {k: v.to_json(base) for k, v in sorted(self.refs.items())},
        }


@dataclass(frozen=True)
class SpanMap:
    """The side table written to ``spans.json`` next to the emitted modules."""

    components: tuple[ComponentSpans, ...]

    def to_json(self, base: Any = None) -> dict[str, Any]:
        return {
            "version": 1,
            "produced_by": "predictable build",
            "components": [c.to_json(base) for c in self.components],
        }

    def index(self, sources: dict[str, str]) -> SpanIndex:
        return SpanIndex(self, sources)


def build_span_map(
    components: Sequence[BuiltComponent],
    files: dict[str, str],
    file_modules: dict[str, str | None],
    reg: Any | None = None,
) -> SpanMap:
    """Locate every emitted component in its `.pir` file and pair it with its Python."""
    by_module: dict[str, list[BuiltComponent]] = {}
    for c in components:
        by_module.setdefault(c.module, []).append(c)

    out: list[ComponentSpans] = []
    for pir_file, module in file_modules.items():
        if module is None:
            continue
        blocks = component_blocks(files[pir_file])
        for c in by_module.get(module, ()):
            start, end = blocks.get(c.name, (0, 0))
            func = _func_of(c, reg)
            out.append(
                ComponentSpans(
                    name=c.name,
                    pir_file=pir_file,
                    pir_line=start,
                    pir_end_line=end,
                    origin=_origin_of(c, func),
                    refs=_refs_of(func),
                )
            )
    return SpanMap(tuple(out))


def _origin_of(c: BuiltComponent, func: Callable[..., Any] | None) -> PySpan | None:
    span = c.meta.get("origin_span")
    if not span:
        return None
    if func is not None:
        precise = _def_span(func)
        if precise is not None:
            return precise
    return PySpan(span["file"], span["line"], span.get("col_start", 0), span.get("col_end", 0))


def _func_of(c: BuiltComponent, reg: Any | None = None) -> Callable[..., Any] | None:
    from .declarations import registry

    active = reg if reg is not None else registry()
    module_reg = active.modules.get(c.module)
    decl = module_reg.components.get(c.name) if module_reg is not None else None
    return getattr(decl, "func", None) if decl is not None else None


def _refs_of(func: Callable[..., Any] | None) -> dict[str, PySpan]:
    if func is None:
        return {}
    spans = body_spans(func)
    # A dependency named only in the signature (the `dependencies are parameters` rule) still has
    # a span: `ast.arg` is not a `Name`, so add the parameter list explicitly.
    spans.update({k: v for k, v in _param_spans(func).items() if k not in spans})
    return spans


def _param_spans(func: Callable[..., Any]) -> dict[str, PySpan]:
    try:
        source = inspect.getsource(func)
        tree = ast.parse(textwrap.dedent(source))
    except (OSError, TypeError, SyntaxError):  # pragma: no cover
        return {}
    file = inspect.getsourcefile(func) or func.__code__.co_filename
    first = func.__code__.co_firstlineno
    indent = _indent_of(source)
    out: dict[str, PySpan] = {}
    for node in ast.walk(tree):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            args = node.args
            for arg in [*args.posonlyargs, *args.args, *args.kwonlyargs]:
                out.setdefault(
                    arg.arg,
                    PySpan(
                        file,
                        first + arg.lineno - 1,
                        arg.col_offset + indent,
                        (arg.end_col_offset or arg.col_offset + len(arg.arg)) + indent,
                    ),
                )
    return out


def component_blocks(text: str) -> dict[str, tuple[int, int]]:
    """``name -> (first line, last line)`` of every `[[component]]` block in canonical text."""
    out: dict[str, tuple[int, int]] = {}
    start: int | None = None
    name: str | None = None
    lines = text.split("\n")
    for i, line in enumerate(lines, start=1):
        if line == "[[component]]":
            if start is not None and name is not None:
                out[name] = (start, i - 1)
            start, name = i, None
        elif line.startswith("[") and not line.startswith("[component."):
            if start is not None and name is not None:
                out[name] = (start, i - 1)
            start, name = None, None
        elif start is not None and name is None and line.startswith('name = "'):
            name = line[len('name = "') : -1]
    if start is not None and name is not None:
        out[name] = (start, len(lines))
    return out


# --------------------------------------------------------------------------------------
# backward: `.pir` span → Python span
# --------------------------------------------------------------------------------------


class SpanIndex:
    """Answers "which Python does this `.pir` byte range belong to?"."""

    def __init__(self, span_map: SpanMap, sources: dict[str, str]) -> None:
        self._sources = sources
        self._by_file: dict[str, list[ComponentSpans]] = {}
        for c in span_map.components:
            self._by_file.setdefault(c.pir_file, []).append(c)

    def component_at(self, file: str, line: int) -> ComponentSpans | None:
        for c in self._by_file.get(file, ()):
            if c.pir_line <= line <= c.pir_end_line:
                return c
        return None

    def lookup(self, file: str, line: int, start: int, end: int) -> tuple[PySpan | None, str]:
        """The Python span for a `.pir` span, plus the `.pir` text it covered.

        The identifier under the span is what decides: an `E0203` pointing at ``qx`` inside an
        expression resolves to the ``qx`` the user wrote, not to the whole function. When the
        span covers something that is not a name the user typed — a `kind`, a `timing` — the
        component's own `def` line is the answer.
        """
        text = self._sources.get(file, "")
        covered = text.encode()[start:end].decode(errors="replace") if text else ""
        token = covered.strip().strip('"')
        component = self.component_at(file, line)
        if component is None:
            return None, covered
        span = component.refs.get(token)
        if span is not None:
            return span, covered
        return component.origin, covered

    def files(self) -> Iterable[str]:
        return self._by_file.keys()
