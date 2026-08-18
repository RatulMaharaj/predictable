"""The DSL's diagnostic model — the Python mirror of ``predictable-diagnostics``.

Every tracer refusal is a :class:`Diagnostic` with ``{code, severity, message, spans,
suggestions, doc_url}``, carried by a :class:`DslError` exception. ``to_json`` produces exactly
the shape the Rust crate serialises, so ``predictable build --json`` can merge DSL diagnostics
and engine diagnostics into one stream without translating either.

Spans point at **Python** source: the file, line and column of the user's own line, captured from
the call stack at the moment the tracer refused. That is the whole point of catching these in
Python rather than in the IR checker (`02-dsl.md` §8 step 3).
"""

from __future__ import annotations

import linecache
import os
import sys
from dataclasses import dataclass
from typing import Any

__all__ = [
    "DOC_BASE",
    "LineCol",
    "Span",
    "Edit",
    "Suggestion",
    "Diagnostic",
    "DslError",
    "capture_span",
    "render",
]

DOC_BASE = "https://predictable.dev/llm/diagnostics/"
"""Base URL the catalogue is published at; ``doc_url`` is this plus ``#<code>``."""

_PACKAGE_DIR = os.path.dirname(os.path.abspath(__file__))


@dataclass(frozen=True)
class LineCol:
    line: int
    column: int

    def to_json(self) -> dict[str, Any]:
        return {"line": self.line, "column": self.column}


@dataclass(frozen=True)
class Span:
    """A byte range in a source file, with the line/column the terminal renderer prints."""

    file: str
    start: int
    end: int
    primary: bool = True
    label: str | None = None
    start_pos: LineCol | None = None
    #: The full source line, kept so the renderer never has to re-read a file that may have
    #: changed underneath it.
    text: str = ""

    def to_json(self) -> dict[str, Any]:
        out: dict[str, Any] = {
            "file": self.file,
            "start": self.start,
            "end": self.end,
            "primary": self.primary,
        }
        if self.label is not None:
            out["label"] = self.label
        if self.start_pos is not None:
            out["start_pos"] = self.start_pos.to_json()
        return out


@dataclass(frozen=True)
class Edit:
    """A literal byte-range replacement — what makes a suggestion mechanically applicable."""

    file: str
    start: int
    end: int
    replacement: str

    def to_json(self) -> dict[str, Any]:
        return {
            "file": self.file,
            "start": self.start,
            "end": self.end,
            "replacement": self.replacement,
        }


@dataclass(frozen=True)
class Suggestion:
    message: str
    edits: tuple[Edit, ...] = ()
    applicability: str = "machine_applicable"

    def to_json(self) -> dict[str, Any]:
        return {
            "message": self.message,
            "edits": [e.to_json() for e in self.edits],
            "applicability": self.applicability,
        }


@dataclass(frozen=True)
class Diagnostic:
    code: str
    message: str
    spans: tuple[Span, ...] = ()
    suggestions: tuple[Suggestion, ...] = ()
    notes: tuple[str, ...] = ()
    severity: str = "error"

    @property
    def doc_url(self) -> str:
        return f"{DOC_BASE}#{self.code}"

    def to_json(self) -> dict[str, Any]:
        out: dict[str, Any] = {
            "code": self.code,
            "severity": self.severity,
            "message": self.message,
            "spans": [s.to_json() for s in self.spans],
            "suggestions": [s.to_json() for s in self.suggestions],
            "doc_url": self.doc_url,
        }
        if self.notes:
            out["notes"] = list(self.notes)
        return out

    def render(self) -> str:
        return render(self)


class DslError(Exception):
    """A DSL diagnostic raised as a Python exception, at the line that caused it.

    The exception message *is* the rendered diagnostic, so a plain traceback in a notebook is
    already the actionable message; ``err.diagnostic.to_json()`` is the machine-readable form.
    """

    def __init__(self, diagnostic: Diagnostic) -> None:
        self.diagnostic = diagnostic
        super().__init__(render(diagnostic))

    @property
    def code(self) -> str:
        return self.diagnostic.code


# --------------------------------------------------------------------------------------
# span capture
# --------------------------------------------------------------------------------------


def capture_span(label: str | None = None, needle: str | None = None) -> Span:
    """The first stack frame outside this package — the user's own line.

    ``needle`` narrows the underline to a token within that line (a component name, an
    operator) when the tracer knows which one is at fault; otherwise the whole trimmed line is
    underlined, which is still enough for an agent to locate and rewrite it.
    """
    frame = sys._getframe(1)
    while frame is not None:
        filename = os.path.abspath(frame.f_code.co_filename)
        if os.path.dirname(filename) != _PACKAGE_DIR:
            break
        frame = frame.f_back
    if frame is None:  # pragma: no cover - only if the whole stack is inside the package
        return Span("<unknown>", 0, 0, True, label, LineCol(0, 0), "")

    filename = frame.f_code.co_filename
    lineno = frame.f_lineno
    text = linecache.getline(filename, lineno).rstrip("\n")
    stripped = text.strip()
    col = text.index(stripped) if stripped else 0
    width = len(stripped)
    if needle and needle in text:
        col = text.index(needle)
        width = len(needle)
    # A file-relative byte offset is not available without reading the whole file; the offset
    # of the line plus the column is, and is what the build step maps into `.pir` spans.
    line_start = sum(
        len(linecache.getline(filename, i)) for i in range(1, lineno) if linecache.getline(filename, i)
    )
    start = line_start + col
    return Span(
        file=os.path.relpath(filename) if os.path.exists(filename) else filename,
        start=start,
        end=start + width,
        primary=True,
        label=label,
        start_pos=LineCol(lineno, col + 1),
        text=text,
    )


# --------------------------------------------------------------------------------------
# rendering
# --------------------------------------------------------------------------------------


def render(diag: Diagnostic) -> str:
    """The terminal form of `02-dsl.md` §10.1 — a header, the source, then the fix."""
    lines = [f"{diag.severity}[{diag.code}]: {diag.message}"]
    for span in diag.spans:
        lines.append("")
        pos = span.start_pos
        if pos is not None:
            lines.append(f"   ┌─ {span.file}:{pos.line}:{pos.column}")
        else:  # pragma: no cover
            lines.append(f"   ┌─ {span.file}")
        if span.text:
            gutter = str(pos.line if pos else "").rjust(3)
            lines.append(f"{gutter} │ {span.text}")
            caret = "^" if span.primary else "-"
            width = max(span.end - span.start, 1)
            pad = " " * ((pos.column - 1) if pos else 0)
            marker = f"    │ {pad}{caret * width}"
            if span.label:
                marker += f" {span.label}"
            lines.append(marker)
        elif span.label:  # pragma: no cover
            lines.append(f"    │ {span.label}")
    for note in diag.notes:
        lines.append("")
        lines.append("  " + note.replace("\n", "\n  "))
    for suggestion in diag.suggestions:
        lines.append("")
        lines.append("  help: " + suggestion.message.replace("\n", "\n  "))
        for edit in suggestion.edits:
            # The replacement is usually quoted in the message already; echo it only when it
            # is not, so the fix appears exactly once.
            if edit.replacement not in suggestion.message:
                lines.append(f"        {edit.replacement}")
    lines.append("")
    lines.append(f"  see {diag.doc_url}")
    return "\n".join(lines)
