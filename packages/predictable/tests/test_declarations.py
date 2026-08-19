"""Declaration-layer behaviour and the `E11xx` / `E13xx` / `E14xx` / `E16xx` catalogue.

Each error test asserts three things, because all three are the product: the **code**, so an agent
can dispatch on it; something specific in the **message**, so a human can act on it without opening
the spec; and, where the fix is mechanical, the **suggested edit**.
"""

import pytest

from predictable import (
    Enum,
    Key,
    ModelPoint,
    Value,
    assumption,
    build,
    key,
    per_mp,
    product,
    registry_scope,
    scalar,
    series,
    t,
    table,
    timeline,
)
from predictable.diagnostics import DslError
from predictable.timing import END, POINT, START
from predictable.units import Count, Factor, Flag, Money, Num, Prob, Rate, Years, annotation_type


@pytest.fixture()
def scope():
    """A fresh registry, pre-loaded with the schema most of these tests read from."""
    with registry_scope() as reg:

        class TermMP(ModelPoint):
            policy_number: str = key()
            entry_age: Years
            sum_assured: Money
            annual_premium: Money

        yield reg


def raises(code):
    """Assert a `DslError` with a given code, and hand the diagnostic back for inspection."""

    class _Ctx:
        def __enter__(self):
            self._cm = pytest.raises(DslError)
            self.excinfo = self._cm.__enter__()
            return self

        def __exit__(self, *exc):
            handled = self._cm.__exit__(*exc)
            if handled:
                assert self.excinfo.value.code == code, (
                    f"expected {code}, got {self.excinfo.value.code}\n"
                    f"{self.excinfo.value}"
                )
                self.diagnostic = self.excinfo.value.diagnostic
                self.rendered = str(self.excinfo.value)
            return handled

    return _Ctx()


# -- §2.1 the decorators -----------------------------------------------------------------


def test_a_component_is_its_function_name_shape_and_annotation(scope):
    @series(timing=END)
    def death_claims(sum_assured: Money, deaths: Count) -> Money:
        """Death outgo in year t."""
        return sum_assured * deaths

    assert death_claims.name == "death_claims"
    assert death_claims.contract == ("Series", "f64", "money", "end")
    assert death_claims.doc == "Death outgo in year t."
    assert death_claims.kind == "Derived"


def test_output_true_makes_the_kind_output(scope):
    @per_mp(output=True)
    def bel() -> Money:
        return 0.0

    assert bel.kind == "Output"


def test_scalar_and_per_mp_have_no_timing(scope):
    @scalar()
    def portfolio_factor() -> Factor:
        return 1.0

    assert portfolio_factor.timing is None
    assert portfolio_factor.shape == "Scalar"
    with pytest.raises(TypeError):
        per_mp(timing=END)


def test_units_carry_dtype_and_unit():
    assert annotation_type(Money) == ("f64", "money")
    assert annotation_type(Years) == ("i64", "years")
    assert annotation_type(Flag) == ("bool", "none")
    assert annotation_type(Num) == ("f64", "none")
    assert annotation_type(Rate.monthly) == ("f64", "rate(monthly)")
    assert annotation_type(bool) == ("bool", "none")
    assert annotation_type(str) == ("str", "none")


def test_e1101_missing_return_annotation(scope):
    with raises("E1101") as caught:

        @series(timing=END)
        def claims(sum_assured: Money):
            return sum_assured

    assert "no return annotation" in caught.diagnostic.message
    assert "never infers a unit" in caught.rendered
    assert "Money | Prob | Count" in caught.rendered


def test_e1102_series_without_timing(scope):
    with raises("E1102") as caught:

        @series()
        def claims(sum_assured: Money) -> Money:
            return sum_assured

    assert "timing" in caught.diagnostic.message
    # The message lists all four tags with a one-line gloss each (§2.5).
    for tag in ("START", "END", "MID", "POINT"):
        assert tag in caught.rendered


# -- §2.2 dependencies are the parameter list --------------------------------------------


def test_e1103_unknown_parameter_suggests_the_near_miss(scope):
    @series(timing=START, init=1.0)
    def num_pols_if(num_pols_if: Count) -> Count:
        return num_pols_if[t - 1]

    @series(timing=START)
    def premium_income(num_pols_iff: Count) -> Money:
        return num_pols_iff

    with raises("E1103") as caught:
        build()

    assert "num_pols_iff" in caught.diagnostic.message
    edit = caught.diagnostic.suggestions[0].edits[0]
    assert edit.replacement == "num_pols_if"
    assert "Components in scope" in caught.rendered


def test_e1103_free_variable_capture_is_an_error(scope):
    @series(timing=START, init=1.0)
    def num_pols_if(num_pols_if: Count) -> Count:
        return num_pols_if[t - 1]

    @series(timing=START)
    def deaths(qx: Prob) -> Count:
        return num_pols_if * qx  # captured, not declared

    @series(timing=END)
    def qx() -> Prob:
        return 0.01

    with raises("E1103") as caught:
        build()

    assert "free variable" in caught.diagnostic.message
    assert "dependencies are exactly its parameters" in caught.rendered


def test_e1104_annotation_contradicts_the_declaration(scope):
    @series(timing=END)
    def qx() -> Prob:
        return 0.01

    @series(timing=END)
    def deaths(qx: Prob) -> Count:
        return qx

    @series(timing=END)
    def death_claims(deaths: Money) -> Money:
        return deaths

    with raises("E1104") as caught:
        build()

    assert "deaths" in caught.diagnostic.message
    assert "you annotated `deaths` as money" in caught.rendered
    assert "declared as count" in caught.rendered
    # Two spans: the annotation, and the declaration it contradicts.
    assert len(caught.diagnostic.spans) == 2
    assert caught.diagnostic.spans[1].primary is False


def test_a_matching_annotation_is_accepted(scope):
    @series(timing=END)
    def qx() -> Prob:
        return 0.01

    @series(timing=END)
    def deaths(qx: Prob) -> Count:
        return qx

    @series(timing=END)
    def death_claims(deaths: Count) -> Money:
        return deaths

    assert build().component("death_claims").dependencies == ("deaths",)


def test_e1105_ambiguous_across_modules(scope):
    # Two libraries, each declaring `qx`. Resolution walks one namespace at a time and takes the
    # first match; two matches at the same level cannot be ordered, so the model would otherwise
    # depend on import order.
    def declare_in(module_path, value):
        def qx() -> Prob:
            return value

        decl = series(timing=END)(qx)
        scope.module(decl.module).components.pop("qx", None)
        decl.module = module_path
        scope.module(module_path).components["qx"] = decl

    declare_in("lib_a", 0.01)
    declare_in("lib_b", 0.02)

    @series(timing=END)
    def deaths(qx: Prob) -> Count:
        return qx

    with raises("E1105") as caught:
        build()

    assert "ambiguous" in caught.diagnostic.message
    assert "lib_a" in caught.diagnostic.message and "lib_b" in caught.diagnostic.message


def test_locals_inline_and_do_not_become_components(scope):
    @series(timing=POINT, init=1.0)
    def reserve(reserve: Money) -> Money:
        return reserve[t - 1]

    @series(timing=END)
    def net_death_strain(sum_assured: Money, reserve: Money, deaths: Count) -> Money:
        strain_per_death = sum_assured - reserve[t - 1]
        return strain_per_death * deaths

    @series(timing=END)
    def deaths() -> Count:
        return 0.0

    model = build()
    assert (
        model.component("net_death_strain").expr.to_pir()
        == "(sum_assured - reserve[t-1]) * deaths"
    )
    assert "strain_per_death" not in [c.name for c in model.components]


# -- §3 modelpoint schema ----------------------------------------------------------------


def test_modelpoint_fields_and_defaults(scope):
    class TermMP(ModelPoint):
        policy_number: str = key()
        entry_age: Years
        sum_assured: Money
        channel: str = "DIRECT"

    fields = {f.name: f for f in TermMP._predictable_fields}
    assert fields["policy_number"].key is True
    assert fields["entry_age"].required is True
    assert fields["channel"].required is False
    assert fields["channel"].default == "DIRECT"
    assert fields["sum_assured"].dtype == "f64" and fields["sum_assured"].unit == "money"


def test_e1301_zero_keys(scope):
    with raises("E1301") as caught:

        class NoKey(ModelPoint):
            entry_age: Years

    assert "exactly one" in caught.diagnostic.message
    assert "policy_number: str = key()" in caught.rendered


def test_e1301_two_keys(scope):
    with raises("E1301") as caught:

        class TwoKeys(ModelPoint):
            policy_number: str = key()
            other_id: str = key()

    assert "2 key() fields" in caught.rendered


def test_e1302_modelpoint_accessed_as_a_value(scope):
    class TermMP(ModelPoint):
        policy_number: str = key()
        sum_assured: Money

    with raises("E1302") as caught:
        TermMP.sum_assured

    assert "not a value" in caught.diagnostic.message
    assert "add `sum_assured` to the parameter list" in caught.rendered


def test_enum_members_compare_by_equality(scope):
    class Gender(Enum):
        M = "M"
        F = "F"

    assert Gender.F.value == "F"
    assert Gender.F == Gender.F
    assert Gender.F != Gender.M
    assert scope.enums["Gender"].values == ("M", "F")
    from predictable.units import annotation_type as at

    assert at(Gender) == ("enum(Gender)", "none")


# -- §4 assumptions ----------------------------------------------------------------------


def test_assumption_names_itself_and_carries_its_unit(scope):
    valuation_rate = assumption(Rate.annual)
    mortality_loading = assumption(Factor, default=1.0)
    per_policy = assumption(Money, shape="PerMP")

    assert valuation_rate.name == "valuation_rate"
    assert valuation_rate.unit == "rate(annual)"
    assert mortality_loading.default == 1.0
    assert per_policy.shape == "PerMP"
    assert set(scope.assumptions) == {"valuation_rate", "mortality_loading", "per_policy"}


def test_assumption_needs_a_unit(scope):
    with pytest.raises(TypeError):
        assumption(object())


# -- §5 timeline -------------------------------------------------------------------------


def test_timeline_inputs_resolve_without_declaration(scope):
    timeline(basis="annual", periods=40)

    @series(timing=START)
    def anniversary_flag(policy_year: Years) -> Flag:
        return policy_year > 1

    model = build()
    assert model.component("anniversary_flag").expr.to_pir() == "policy_year > 1"
    assert model.timeline.periods == 40


def test_e1401_redefining_a_timeline_input(scope):
    with raises("E1401") as caught:

        @series(timing=START)
        def policy_year() -> Years:
            return 1

    assert "timeline input" in caught.diagnostic.message
    assert "policy_year_capped" in caught.rendered


def test_e1402_dividing_a_rate_to_change_basis(scope):
    @series(timing=POINT)
    def monthly_rate(valuation_rate: Rate.annual) -> Rate.monthly:
        return valuation_rate / 12

    assumption(Rate.annual)  # placeholder so the name resolves
    scope.assumptions["valuation_rate"] = scope.assumptions.pop(
        next(iter(scope.assumptions))
    ).__class__(name="valuation_rate", dtype="f64", unit="rate(annual)")

    with raises("E1402") as caught:
        build()

    assert "to_monthly(valuation_rate)" in caught.rendered
    assert "nominal_to_periodic(valuation_rate, 12)" in caught.rendered


# -- §6 tables ---------------------------------------------------------------------------


def test_table_declaration_becomes_ir(scope):
    class Gender(Enum):
        M = "M"
        F = "F"

    sa8990 = table(
        source="tables/sa8990.csv",
        keys=[Key("age", int, policy="clamp"), Key("gender", Gender)],
        values=[Value("qx", Prob)],
    )
    assert sa8990.to_ir() == {
        "name": "sa8990",
        "keys": [
            {"name": "age", "dtype": "i64", "policy": "clamp"},
            {"name": "gender", "dtype": "enum(Gender)", "policy": "exact"},
        ],
        "values": [{"name": "qx", "dtype": "f64", "unit": "prob"}],
        "on_missing": "error",
        "source": "tables/sa8990.csv",
    }


def test_enum_keys_are_exact_only(scope):
    class Gender(Enum):
        M = "M"
        F = "F"

    with pytest.raises(ValueError, match="unordered"):
        table(
            source="t.csv",
            keys=[Key("gender", Gender, policy="clamp")],
            values=[Value("qx", Prob)],
        )


def test_multi_value_table_selects_its_column(scope):
    rates = table(
        source="rates.csv",
        keys=[Key("policy_year", int, policy="step")],
        values=[Value("lapse_pa", Prob), Value("surrender_pa", Prob)],
    )

    @series(timing=END)
    def wx(policy_year: Years, rates=rates) -> Prob:
        return rates(policy_year).lapse_pa

    assert build().component("wx").expr.to_pir() == "rates.lapse_pa@(policy_year)"


def test_e1106_table_looked_up_but_not_in_the_signature(scope):
    lapse_rates = table(
        source="lapses.csv", keys=[Key("policy_year", int)], values=[Value("lapse_pa", Prob)]
    )

    @series(timing=END)
    def wx(policy_year: Years) -> Prob:
        return lapse_rates(policy_year)

    with raises("E1106") as caught:
        build()

    assert "not in its signature" in caught.diagnostic.message
    assert "Add `lapse_rates=lapse_rates`" in caught.rendered


def test_e1106_table_in_the_signature_but_never_used(scope):
    lapse_rates = table(
        source="lapses.csv", keys=[Key("policy_year", int)], values=[Value("lapse_pa", Prob)]
    )

    @series(timing=END)
    def wx(policy_year: Years, lapse_rates=lapse_rates) -> Prob:
        return policy_year * 0.0

    with raises("E1106") as caught:
        build()

    assert "never looks it up" in caught.diagnostic.message
    assert "Remove `lapse_rates=lapse_rates`" in caught.rendered


# -- §7.3 generated components -----------------------------------------------------------


def test_name_suffix_is_the_only_composed_name(scope):
    bands = [("a", 0.0, 100.0), ("b", 100.0, 1e9)]
    for band, lo, hi in bands:

        @series(timing=START, name_suffix=band)
        def expense_band(sum_assured: Money, lo=lo, hi=hi) -> Money:
            from predictable.fn import all_, when

            return when(all_(sum_assured >= lo, sum_assured < hi), 1.0, 0.0)

    model = build()
    names = [c.name for c in model.components]
    assert names == ["expense_band_a", "expense_band_b"]
    assert model.component("expense_band_a").expr.to_pir() == (
        "if sum_assured >= 0.0 and sum_assured < 100.0 then 1.0 else 0.0"
    )
    # The loop that produced it is recorded, so a reader of the IR can find it (§7.3).
    generated = model.component("expense_band_a").meta["generated_from"]
    assert generated["iteration"] == "a"
    assert generated["file"].endswith("test_declarations.py")
    assert model.component("expense_band_a").meta["authored_by"] == "dsl"


# -- §8.2 the stage-2 back channel -------------------------------------------------------


def test_e1207_stage2_value_read_at_time_t(scope):
    @series(timing=START)
    def premium_income(sum_assured: Money) -> Money:
        return sum_assured

    @per_mp(output=True)
    def bel(premium_income: Money) -> Money:
        from predictable.fn import sum_

        return sum_(premium_income)

    @series(timing=POINT)
    def reserve(bel: Money) -> Money:
        return bel

    with raises("E1207") as caught:
        build()

    assert "bel" in caught.diagnostic.message


def test_stage2_may_seed_init(scope):
    @series(timing=START)
    def premium_income(sum_assured: Money) -> Money:
        return sum_assured

    @per_mp(output=True)
    def bel(premium_income: Money) -> Money:
        from predictable.fn import sum_

        return sum_(premium_income)

    @series(timing=POINT, init=bel, output=True)
    def reserve(reserve: Money) -> Money:
        return reserve[t - 1] * 1.05

    model = build()
    assert model.component("reserve").init.to_pir() == "bel"
    assert model.component("bel").stage == 2


# -- §11 the product manifest ------------------------------------------------------------


def test_e1601_output_manifest_disagrees(scope):
    @per_mp(output=True)
    def bel(sum_assured: Money) -> Money:
        return sum_assured

    p = product("term", modules=[__name__], outputs=["bel", "reserve"])

    with raises("E1601") as caught:
        build(p)

    assert "disagrees with the model" in caught.diagnostic.message
    assert "listed in `outputs=` but not marked `output=True`: reserve" in caught.rendered


def test_e1601_reports_an_unlisted_output(scope):
    @per_mp(output=True)
    def bel(sum_assured: Money) -> Money:
        return sum_assured

    p = product("term", modules=[__name__], outputs=[])
    with raises("E1601") as caught:
        build(p)

    assert "marked `output=True` but absent from `outputs=`: bel" in caught.rendered


def test_product_to_ir(scope):
    @per_mp(output=True)
    def bel(sum_assured: Money) -> Money:
        return sum_assured

    p = product(
        "term",
        modules=[__name__],
        outputs=["bel"],
        key_field="policy_number",
        assumptions="assumptions/base",
        doc="UK level term.",
    )
    model = build(p)
    assert model.to_ir()["product"] == {
        "name": "term",
        "modules": [__name__],
        "outputs": ["bel"],
        "key_field": "policy_number",
        "assumptions": "assumptions/base",
        "doc": "UK level term.",
    }


# -- lints -------------------------------------------------------------------------------


def test_w1101_declared_but_never_read(scope):
    @series(timing=END)
    def orphan(sum_assured: Money) -> Money:
        return sum_assured

    model = build()
    assert [w.code for w in model.warnings] == ["W1101"]
    assert model.warnings[0].severity == "warning"
    assert "never read and is not an output" in model.warnings[0].message
    # A warning never stops the build.
    assert model.component("orphan").expr.to_pir() == "sum_assured"
