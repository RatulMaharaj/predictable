"""The text the tracer prints is the text ``predictable fmt`` prints.

The tracer's canonical printer is a Python re-implementation of ``predictable-fmt``'s
``expr_fmt.rs``. Two implementations of one rule drift unless something compares them, so this
test feeds every expression the tracer produces through the **real** formatter — the ``fmt``
subcommand of ``predictable-cli`` — and requires it to come back unchanged. A `.pir` file that
``fmt`` would rewrite is a `.pir` file ``predictable build --check`` would fail on.

Skipped, rather than failed, when the CLI has not been built: ``cargo build -p predictable-cli``.
"""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest

from test_worked_model import EXPECTED, _params  # noqa: E402

from predictable import trace

REPO = Path(__file__).resolve().parents[3]
CLI = REPO / "target" / "debug" / "predictable"

pytestmark = pytest.mark.skipif(
    not CLI.exists(), reason="predictable-cli not built (cargo build -p predictable-cli)"
)

HEADER = """format = "pir/1"
module = "parity"
"""

COMPONENT = """
[[component]]
name = "c{index}"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "{expr}"
"""


def _module_text(expressions: list[str]) -> str:
    body = "".join(
        COMPONENT.format(index=i, expr=e.replace("\\", "\\\\").replace('"', '\\"'))
        for i, e in enumerate(expressions)
    )
    return HEADER + body


def test_every_traced_expression_is_already_in_canonical_form():
    expressions = [trace(func, _params(func)).pir for func, _ in EXPECTED]
    source = _module_text(expressions)
    result = subprocess.run(
        [str(CLI), "fmt", "--stdout", "-"],
        input=source,
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout == source, "predictable fmt rewrote the tracer's own output"
