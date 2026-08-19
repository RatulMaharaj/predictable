"""Tracing real component bodies — the ones `02-dsl.md` §11 uses as the worked model."""

from __future__ import annotations

from predictable import Param, t, trace, trace_init
from predictable.fn import compound, npv, retime, sum_, when
from predictable.ir import At, Lag, Ref
from predictable.proxy import TableProxy
from predictable.timing import MID


def pir(func, params=(), **kwargs):
    return trace(func, [Param(p) if isinstance(p, str) else p for p in params], **kwargs).pir


def test_a_product_of_two_parameters():
    def death_claims(sum_assured, deaths):
        return sum_assured * deaths

    assert pir(death_claims, ["sum_assured", "deaths"]) == "sum_assured * deaths"


def test_local_variables_are_inlined_not_promoted_to_components():
    def net_death_strain(sum_assured, reserve, deaths):
        strain_per_death = sum_assured - reserve[t - 1]
        return strain_per_death * deaths

    result = trace(net_death_strain, [Param("sum_assured"), Param("reserve"), Param("deaths")])
    assert result.pir == "(sum_assured - reserve[t-1]) * deaths"
    assert result.dependencies == ("sum_assured", "reserve", "deaths")


def test_a_local_used_twice_is_spliced_in_twice():
    def spread(a, b):
        half = (a - b) / 2
        return half + half

    assert pir(spread, ["a", "b"]) == "(a - b) / 2 + (a - b) / 2"


def test_self_reference_with_a_lag_is_legal_and_recursion_is_visible():
    def num_pols_if(num_pols_if, qx, wx, in_term):
        return num_pols_if[t - 1] * (1 - qx[t - 1]) * (1 - wx[t - 1]) * when(in_term, 1.0, 0.0)

    result = trace(num_pols_if, [Param(n) for n in ("num_pols_if", "qx", "wx", "in_term")])
    assert result.pir == (
        "num_pols_if[t-1] * (1 - qx[t-1]) * (1 - wx[t-1]) * (if in_term then 1.0 else 0.0)"
    )
    assert "num_pols_if" in result.dependencies


def test_t_is_a_value_as_well_as_an_index():
    def age(entry_age):
        return entry_age + t

    assert pir(age, ["entry_age"]) == "entry_age + t"

    def in_term(policy_term):
        return t < policy_term

    assert pir(in_term, ["policy_term"]) == "t < policy_term"


def test_absolute_indexing_lowers_to_at():
    def issue_premium(premium_rate):
        return premium_rate[0]

    result = trace(issue_premium, [Param("premium_rate")])
    assert result.expr == At("premium_rate", 0)
    assert result.pir == "premium_rate[0]"


def test_a_python_int_lag_is_constant_folded():
    LAG = 3

    def lagged(x):
        return x[t - LAG]

    assert trace(lagged, [Param("x")]).expr == Lag("x", 3)


def test_reflected_operators_work_from_the_constant_side():
    def survival(qx):
        return 1 - qx

    assert pir(survival, ["qx"]) == "1 - qx"


def test_table_lookup_through_the_default_argument_idiom():
    sa8990 = TableProxy("sa8990", ("qx",))

    def qx(age, gender, smoker, mortality_loading, sa8990=sa8990):
        return sa8990(age, gender, smoker) * mortality_loading

    result = trace(qx, [Param(n) for n in ("age", "gender", "smoker", "mortality_loading")])
    assert result.pir == "sa8990@(age, gender, smoker) * mortality_loading"
    assert result.tables == ("sa8990",)


def test_multi_value_tables_select_the_column_by_attribute():
    rates = TableProxy("rates", ("qx", "wx"))

    def decrement(age, rates=rates):
        return rates(age).wx

    assert pir(decrement, ["age"]) == "rates.wx@(age)"


def test_retime_prints_its_timing_tag_bare():
    def net_cashflow(premium_income, death_claims):
        return retime(premium_income, MID) - retime(death_claims, MID)

    assert pir(net_cashflow, ["premium_income", "death_claims"]) == (
        "retime(premium_income, mid) - retime(death_claims, mid)"
    )


def test_a_builtin_call_with_t_as_an_argument():
    def renewal_expenses(renewal_expense_pa, expense_inflation, num_pols_if, in_term):
        return (
            renewal_expense_pa
            * compound(expense_inflation, t)
            * num_pols_if
            * when(in_term, 1.0, 0.0)
        )

    assert pir(
        renewal_expenses,
        ["renewal_expense_pa", "expense_inflation", "num_pols_if", "in_term"],
    ) == (
        "renewal_expense_pa * compound(expense_inflation, t) * num_pols_if "
        "* (if in_term then 1.0 else 0.0)"
    )


def test_an_aggregate_makes_the_component_stage_2():
    def pv_premiums(premium_income, disc_factor):
        return npv(premium_income, disc_factor)

    result = trace(pv_premiums, [Param("premium_income"), Param("disc_factor")])
    assert result.pir == "npv(premium_income, disc_factor)"
    assert result.stage2 is True


def test_a_body_without_an_aggregate_is_stage_1():
    def deaths(num_pols_if, qx):
        return num_pols_if * qx

    assert trace(deaths, [Param("num_pols_if"), Param("qx")]).stage2 is False


def test_a_predicated_aggregate_carries_its_predicate():
    def early_claims(death_claims, in_term):
        return sum_(death_claims, in_term)

    assert pir(early_claims, ["death_claims", "in_term"]) == "sum(death_claims, in_term)"


def test_a_literal_body_is_lifted_to_a_literal_expression():
    def zero():
        return 0.0

    assert trace(zero).pir == "0.0"


def test_init_lambdas_trace_exactly_like_a_body():
    result = trace_init(lambda annual_premium: annual_premium * 1.0, [Param("annual_premium")])
    assert result.pir == "annual_premium * 1.0"


def test_a_stage_2_value_may_seed_init():
    result = trace_init(lambda bel: bel, [Param("bel", shape="PerMP", stage2=True)])
    assert result.expr == Ref("bel")


def test_generated_declarations_fold_their_python_defaults_into_literals():
    def expense_band(sum_assured, lo=10000, hi=50000):
        from predictable.fn import all_

        return when(all_(sum_assured >= lo, sum_assured < hi), 1.0, 0.0)

    assert pir(expense_band, ["sum_assured"]) == (
        "if sum_assured >= 10000 and sum_assured < 50000 then 1.0 else 0.0"
    )


def test_dependencies_are_first_appearance_ordered_and_deduplicated():
    def reserve(reserve, premium_income, death_claims, valuation_rate):
        return (reserve[t - 1] + premium_income[t - 1] - death_claims[t - 1]) * (
            1 + valuation_rate
        )

    result = trace(
        reserve,
        [Param(n) for n in ("reserve", "premium_income", "death_claims", "valuation_rate")],
    )
    assert result.dependencies == (
        "reserve",
        "premium_income",
        "death_claims",
        "valuation_rate",
    )


def test_the_trace_result_is_also_available_as_ir_json():
    def deaths(num_pols_if, qx):
        return num_pols_if * qx

    assert trace(deaths, [Param("num_pols_if"), Param("qx")]).to_json() == {
        "node": "Binary",
        "op": "mul",
        "lhs": {"node": "Ref", "name": "num_pols_if"},
        "rhs": {"node": "Ref", "name": "qx"},
    }
