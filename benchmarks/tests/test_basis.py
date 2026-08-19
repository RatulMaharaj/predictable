"""The shared inputs. If these drift, every engine drifts together — or none does."""

from __future__ import annotations

from bench import basis


def test_assumption_sets_come_from_the_committed_base_pir():
    """A hard-coded copy would let the benchmark value on a stale basis undetected."""
    text = (basis.MODELS / "term_annual" / "base.pir").read_text()
    assert "valuation_rate = 0.035" in text
    assert basis.TERM_BASIS["valuation_rate"] == 0.035
    # Metadata is not basis and must not leak into the assumption set.
    assert "format" not in basis.TERM_BASIS
    assert "assumption_set" not in basis.TERM_BASIS
    assert "model_module" not in basis.TERM_BASIS


def test_the_ifrs17_basis_extends_the_savings_basis():
    for key, value in basis.SAVINGS_BASIS.items():
        assert basis.IFRS17_BASIS[key] == value
    assert {"ra_pct", "locked_in_rate"} <= set(basis.IFRS17_BASIS)


def test_tables_are_the_reference_corpus_tables():
    """The mortality table is keyed (sex, smoker) and clamped by age, 18..120."""
    table = basis.mortality_table()
    assert set(table) == {("M", True), ("M", False), ("F", True), ("F", False)}
    for column in table.values():
        assert len(column) == basis.MAX_AGE - basis.MIN_AGE + 1
        # `make_fixtures` caps the Makeham rate at 1.0, which it reaches in the 110s.
        assert all(0.0 < q <= 1.0 for q in column)
        assert column == sorted(column), "mortality must be monotone in age"
    # A male smoker is never lighter than a female non-smoker, and is heavier below the cap.
    male_smoker, female_non = table[("M", True)], table[("F", False)]
    assert all(m >= f for m, f in zip(male_smoker, female_non))
    assert male_smoker[0] > female_non[0]


def test_lapse_and_surrender_tables_are_one_based_on_policy_year():
    assert basis.lapse_table()[1] == 0.145
    assert basis.surrender_table()[1] == 0.070
    assert basis.surrender_table()[8] == 0.0


def test_modelpoint_generation_is_deterministic_and_scales():
    a = basis.term_modelpoints(25)
    b = basis.term_modelpoints(25)
    assert a == b
    big = basis.term_modelpoints(1000)
    assert len(big) == 1000
    assert big[:25] == a, "growing the portfolio must not renumber the existing policies"
    assert big[0]["policy_number"] == "TA00001"
    assert basis.savings_modelpoints(20)[0]["policy_number"] == "SV00001"
