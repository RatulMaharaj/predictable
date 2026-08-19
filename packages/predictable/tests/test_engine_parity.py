"""Parity with the Rust crates the tracer has to agree with.

The tracer produces IR for the engine to consume, so two things must not drift:

* the builtin set, against ``crates/predictable-ir/src/builtins.rs``;
* the diagnostic codes, against the registry in ``crates/predictable-diagnostics``, which is
  itself CI-checked against the published docs anchors.

Both are read out of the Rust source rather than restated, so a change on either side fails here
instead of at ``predictable build`` time in a user's model.
"""

from __future__ import annotations

import re
from pathlib import Path

import pytest

from predictable.ir import BUILTINS

REPO = Path(__file__).resolve().parents[3]
BUILTINS_RS = REPO / "crates" / "predictable-ir" / "src" / "builtins.rs"
REGISTRY_RS = REPO / "crates" / "predictable-diagnostics" / "src" / "registry.rs"

pytestmark = pytest.mark.skipif(
    not BUILTINS_RS.exists() or not REGISTRY_RS.exists(),
    reason="running outside the predictable repository checkout",
)


def _rust_builtins() -> set[str]:
    text = BUILTINS_RS.read_text()
    body = text.split("pub const BUILTINS", 1)[1].split("];", 1)[0]
    return set(re.findall(r'"([a-z_0-9]+)"', body))


def _registry_codes() -> set[str]:
    return set(re.findall(r'"([EWPNH][0-9]{4})"', REGISTRY_RS.read_text()))


#: Every code this package can raise. Kept explicit so adding one is a deliberate act.
DSL_CODES = {
    "E1201",
    "E1202",
    "E1203",
    "E1204",
    "E1205",
    "E1206",
    "E1207",
    "E1302",
    "E1402",
}


def test_the_python_builtin_set_is_the_rust_builtin_set():
    assert BUILTINS == _rust_builtins()


def test_every_code_the_tracer_raises_is_in_the_engine_registry():
    assert DSL_CODES <= _registry_codes()


def test_every_declared_code_is_actually_reachable_from_the_error_module():
    from predictable import errors

    source = (Path(errors.__file__)).read_text()
    for code in DSL_CODES:
        assert f'code="{code}"' in source, code


def test_the_docs_catalogue_has_an_anchor_for_every_code_the_tracer_raises():
    catalogue = REPO / "docs" / "llm" / "diagnostics.md"
    if not catalogue.exists():  # pragma: no cover - docs are part of the repo
        pytest.skip("diagnostics catalogue not present")
    anchors = set(re.findall(r'<a id="([EWPNH][0-9]{4})">', catalogue.read_text()))
    assert DSL_CODES <= anchors
