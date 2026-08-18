"""Shared fixtures: the term-assurance model of `03-engine.md` §11.2, as `.pir` text."""

import pytest

TERM = """
format = "pir/1"
module = "term"

[timeline]
basis = "annual"
periods = 3
origin = "policy"
valuation_date = 2026-06-30

[[modelpoint_field]]
name = "policy_number"
dtype = "str"
required = true
key = true

[[modelpoint_field]]
name = "product_code"
dtype = "str"
required = true

[[modelpoint_field]]
name = "sum_assured"
dtype = "f64"
unit = "money"
required = true

[[modelpoint_field]]
name = "premium"
dtype = "f64"
unit = "money"
required = true

[[modelpoint_field]]
name = "q"
dtype = "f64"
unit = "prob"
required = true

[[component]]
name = "survivors"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "count"
timing = "start"
init = "1.0"
expr = "survivors[t-1] * (1 - q)"

[[component]]
name = "claims"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "survivors * q * sum_assured"

[[component]]
name = "premium_income"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "start"
expr = "survivors * premium"

[[component]]
name = "net_cashflow"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "claims - premium_income"

[[component]]
name = "bel"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "sum(net_cashflow)"
"""

# A model whose `ratio` divides by a modelpoint field, so a zero traps (`E0902`).
TRAPPING = """
format = "pir/1"
module = "trap"

[timeline]
basis = "annual"
periods = 2
origin = "policy"
valuation_date = 2026-06-30

[[modelpoint_field]]
name = "policy_number"
dtype = "str"
required = true
key = true

[[modelpoint_field]]
name = "numerator"
dtype = "f64"
unit = "money"
required = true

[[modelpoint_field]]
name = "exposure"
dtype = "f64"
unit = "count"
required = true

[[component]]
name = "ratio"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "numerator / exposure"
"""

MODELPOINTS = """policy_number,product_code,sum_assured,premium,q
POL1,TERM_UK,100000,900,0.01
POL2,TERM_UK,250000,1500,0.02
POL3,TERM_IE,50000,400,0.005
POL4,TERM_IE,75000,600,0.03
"""


def run_pir(*, on_trap="abort", emit="outputs", retain="ring", chunk_size=1024, extra=""):
    return f"""
format = "pir/1"

[run]
product = "term"
modelpoints = "modelpoints.csv"
out = "out"
emit = "{emit}"
retain = "{retain}"
on_trap = "{on_trap}"

[run.exec]
chunk_size = {chunk_size}
{extra}
"""


@pytest.fixture()
def project(tmp_path):
    """A on-disk project: the term model, a run file and a modelpoint CSV."""
    (tmp_path / "term.pir").write_text(TERM)
    (tmp_path / "run.pir").write_text(run_pir())
    (tmp_path / "modelpoints.csv").write_text(MODELPOINTS)
    return tmp_path


@pytest.fixture()
def term_program():
    from predictable_engine import Program

    return Program.from_sources({"term.pir": TERM})
