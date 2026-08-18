"""The library layer (`02-dsl.md` §7): `extends`, `@override`, `@abstract`, `@final`.

This is the section that decides whether a company can adopt the tool: a central library team and
product teams that specialise it without forking. Every rule here is enforced at build, and the
tests below are the enforcement.

The library lives in ``tests/acme_lib`` and is re-imported into a fresh registry for each test, the
way ``predictable build`` imports a product's declared module list.
"""

import importlib
import sys

import pytest

from predictable import (
    Library,
    ModelPoint,
    build,
    extends,
    key,
    override,
    per_mp,
    product,
    registry_scope,
    series,
)
from predictable.diagnostics import DslError
from predictable.fn import when
from predictable.timing import END, START
from predictable.units import Factor, Money, Years


@pytest.fixture()
def base_life():
    """The library module that declares an `@abstract` hook."""
    return importlib.import_module("acme_lib.base_life")


@pytest.fixture()
def acme():
    """A fresh registry with `acme_lib.mortality` imported into it."""
    with registry_scope():
        for name in ("acme_lib.mortality", "acme_lib.base_life"):
            sys.modules.pop(name, None)
        module = importlib.import_module("acme_lib.mortality")

        class TermMP(ModelPoint):
            policy_number: str = key()
            sum_assured: Money

        yield module


# -- §7.1 distribution -------------------------------------------------------------------


def test_library_names_its_modules():
    lib = importlib.import_module("acme_lib").LIBRARY
    assert isinstance(lib, Library)
    assert (lib.name, lib.version) == ("acme", "3.2.0")
    assert lib.modules == ("acme_lib.mortality",)


# -- §7.2 extends ------------------------------------------------------------------------


def test_extends_copies_components_into_the_module(acme):
    extends(acme)

    model = build()
    names = [c.name for c in model.components]
    assert "qx_loading" in names
    # Copies, not links: the inherited component is built into this product's own IR, so the
    # engine never resolves inheritance (§7.2).
    assert model.component("qx_loading").expr.to_pir() == "1.0"


def test_extends_accepts_a_library(acme):
    extends(importlib.import_module("acme_lib").LIBRARY)
    assert "qx_loading" in [c.name for c in build().components]


def test_override_replaces_and_records_the_provenance(acme):
    extends(acme)

    @override(acme.qx_loading)
    @series(timing=END)
    def qx_loading(policy_year: Years) -> Factor:
        """Term-2026 uses a select loading; the library's flat 1.0 does not apply."""
        return when(policy_year <= 5, 0.75, 1.0)

    model = build()
    built = model.component("qx_loading")
    assert built.expr.to_pir() == "if policy_year <= 5 then 0.75 else 1.0"
    # Recorded so the explorer can render the library-vs-product delta (§7.2).
    assert "override:acme_lib.mortality.qx_loading" in built.tags
    assert built.meta["overrides"] == {
        "library": "acme_lib.mortality",
        "component": "qx_loading",
    }
    # And exactly one `qx_loading` survives into the IR.
    assert [c.name for c in model.components].count("qx_loading") == 1


def test_e1501_shadowing_without_override(acme):
    extends(acme)

    @series(timing=END)
    def qx_loading(policy_year: Years) -> Factor:
        return 0.75

    with pytest.raises(DslError) as excinfo:
        build()

    assert excinfo.value.code == "E1501"
    rendered = str(excinfo.value)
    assert "shadows an inherited component" in rendered
    assert "@override(acme_lib.mortality.qx_loading)" in rendered
    assert "qx_loading_select" in rendered  # the "give it a different name" branch


@pytest.mark.parametrize(
    "kwargs,changed",
    [
        ({"timing": START}, "timing"),
        ({"timing": END, "unit": "money"}, "unit"),
        ({"timing": END, "dtype": "i64"}, "dtype"),
    ],
)
def test_e1502_override_changes_the_contract(acme, kwargs, changed):
    extends(acme)

    with pytest.raises(DslError) as excinfo:

        @override(acme.qx_loading)
        @series(**kwargs)
        def qx_loading() -> Factor:
            return 0.75

    assert excinfo.value.code == "E1502"
    assert changed in str(excinfo.value)


def test_e1502_override_changing_shape(acme):
    extends(acme)

    with pytest.raises(DslError) as excinfo:

        @override(acme.qx_loading)
        @per_mp()
        def qx_loading() -> Factor:
            return 0.75

    assert excinfo.value.code == "E1502"
    assert "shape: base is Series, override is PerMP" in str(excinfo.value)


def test_e1504_override_of_a_final_component(acme):
    extends(acme)

    with pytest.raises(DslError) as excinfo:

        @override(acme.statutory_margin)
        @series(timing=END)
        def statutory_margin() -> Factor:
            return 1.0

    assert excinfo.value.code == "E1504"
    assert "regulatory or" in str(excinfo.value)
    assert "statutory_margin_local" in str(excinfo.value)


def test_e1203_undischarged_abstract(acme, base_life):
    extends(base_life)
    p = product("term_2026", modules=[__name__, "acme_lib.base_life"], outputs=[])

    with pytest.raises(DslError) as excinfo:
        build(p)

    # `@abstract` is DSL-only: there is no `Kind::Abstract`, and the build fails before any
    # `.pir` is written (`01-ir.md` §13 Q10).
    assert excinfo.value.code == "E1203"
    rendered = str(excinfo.value)
    assert "unimplemented abstract component" in rendered
    assert "surrender_value(policy_year: years, sum_assured: money) -> money" in rendered
    assert "@series(timing=END)" in rendered
    assert "no `.pir` is written" in rendered


def test_an_overridden_abstract_discharges_the_product(acme, base_life):
    extends(base_life)

    @override(base_life.surrender_value)
    @series(timing=END, output=True)
    def surrender_value(policy_year: Years, sum_assured: Money) -> Money:
        """An explicit zero is auditable; a missing component is not."""
        return sum_assured * 0.0

    p = product("term_2026", modules=[__name__, "acme_lib.base_life"], outputs=["surrender_value"])
    model = build(p)
    assert model.component("surrender_value").expr.to_pir() == "sum_assured * 0.0"
    assert model.component("surrender_value").kind == "Output"


def test_final_and_abstract_are_recorded_on_the_declaration(acme):
    assert acme.statutory_margin.is_final is True
    assert importlib.import_module("acme_lib.base_life").surrender_value.is_abstract is True
    assert acme.qx_loading.is_final is False
