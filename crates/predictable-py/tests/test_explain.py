"""`plan.explain(...)`: the provenance trace of one cell (`04-verify.md` §3).

The property every test here turns on is the one that makes a trace evidence
rather than commentary: **the replay reproduces the run**. If it did not, the
engine raises `E0901` and these calls would fail — so a passing test is itself
the assertion that the tree's arithmetic is the projection's arithmetic.
"""

import pytest

from predictable_engine import Program

from conftest import MODELPOINTS, TERM, run_pir


@pytest.fixture()
def plan(project):
    return Program.from_pir(str(project)).plan()


def hand_computed(sum_assured, premium, q, periods=3):
    """The term model, by hand — the oracle, not a snapshot of the engine."""
    survivors = 1.0
    claims, cashflows = [], []
    for _ in range(periods + 1):
        claims.append(survivors * q * sum_assured)
        cashflows.append(claims[-1] - survivors * premium)
        survivors = survivors * (1 - q)
    return claims, cashflows


def test_a_trace_reports_the_value_the_run_reports(plan):
    run = plan.run()
    bels = {
        r["mp_key"]: r["value"]
        for r in run.to_arrow().to_pylist()
        if r["component"] == "term.bel"
    }
    for key in bels:
        trace = plan.explain("bel", key)
        assert trace.value == bels[key], key
        assert trace.component == "term.bel"


def test_a_series_needs_a_t_and_matches_the_hand_computation(plan):
    claims, _ = hand_computed(100000, 900, 0.01)
    for t, expected in enumerate(claims):
        trace = plan.explain("claims", "POL1", t)
        assert trace.value == pytest.approx(expected, rel=1e-12)


def test_the_json_is_the_normative_document(plan):
    trace = plan.explain("claims", "POL2", 2, depth=-1)
    doc = trace.json
    assert doc["format"] == "pvf/1"
    assert doc["kind"] == "trace"
    assert doc["root"]["node"] == "Component"
    assert doc["root"]["id"] == "term.claims"
    assert doc["root"]["t"] == 2
    assert doc["root"]["modelpoint"]["key"] == "POL2"
    assert doc["root"]["value"] == trace.value
    # Every fact in the text is in the JSON: the text is a projection.
    assert isinstance(trace.text, str)
    assert "term.claims[t=2]" in trace.text
    assert trace.text == trace.to_text(100)


def test_a_ref_expands_into_the_referenced_components_tree(plan):
    trace = plan.explain("claims", "POL1", 3, depth=-1)

    def walk(node):
        yield node
        for child in node.get("children", []):
            yield from walk(child)

    nodes = list(walk(trace.json["root"]))
    refs = [n for n in nodes if n["node"] == "Ref" and n["ref"] == "term.survivors"]
    assert refs, "no survivors Ref"
    assert refs[0]["children"], "a Ref must carry the referenced component's tree"
    assert refs[0]["children"][0]["node"] == "Component"
    assert refs[0]["children"][0]["value"] == refs[0]["value"]
    # Full depth bottoms out in Input and Lit leaves.
    leaves = [n for n in nodes if not n.get("children")]
    assert {n["node"] for n in leaves} <= {"Input", "Lit", "Agg"}


def test_agg_terms_are_present_and_sum_exactly(plan):
    trace = plan.explain("bel", "POL3", depth=-1)

    def walk(node):
        yield node
        for child in node.get("children", []):
            yield from walk(child)

    aggs = [n for n in walk(trace.json["root"]) if n["node"] == "Agg"]
    assert len(aggs) == 1
    agg = aggs[0]
    assert agg["op"] == "sum"
    assert [term["t"] for term in agg["terms"]] == [0, 1, 2, 3]
    total = 0.0
    for term in agg["terms"]:
        total += term["contribution"]
    assert total == agg["value"] == trace.value


def test_depth_shrinks_the_tree_but_never_the_number(plan):
    shallow = plan.explain("claims", "POL1", 3, depth=0)
    full = plan.explain("claims", "POL1", 3, depth=-1)
    assert shallow.value == full.value
    assert shallow.node_count < full.node_count
    assert shallow.json["truncated"]["elided_nodes"] > 0


def test_expand_beats_depth_for_named_components(plan):
    trace = plan.explain("claims", "POL1", 3, depth=0, expand=["survivors"])

    def walk(node):
        yield node
        for child in node.get("children", []):
            yield from walk(child)

    refs = [
        n
        for n in walk(trace.json["root"])
        if n["node"] == "Ref" and n["ref"] == "term.survivors"
    ]
    assert refs and refs[0].get("children")


def test_init_at_t_zero_is_reported_as_such(plan):
    trace = plan.explain("survivors", "POL1", 0, depth=-1)
    assert trace.value == 1.0
    assert trace.json["root"]["resolution"] == "init"
    assert [n["code"] for n in trace.notes] == ["N0302"]


def test_a_permp_component_refuses_a_t(plan):
    with pytest.raises(Exception) as excinfo:
        plan.explain("bel", "POL1", 0)
    assert "not a Series" in str(excinfo.value)


def test_an_unknown_modelpoint_is_named(plan):
    with pytest.raises(Exception) as excinfo:
        plan.explain("bel", "NOPE")
    assert "NOPE" in str(excinfo.value)


def test_repr_and_notebook_rendering_carry_the_same_text(plan):
    trace = plan.explain("bel", "POL1")
    assert str(trace) == trace.text
    assert "Trace" in repr(trace)
    assert "<pre" in trace._repr_html_()


def test_explaining_does_not_perturb_a_run(project):
    """A trace is a separate single-lane projection, so a run before and after
    one is bit-identical. That is why there is no tracing flag on `run()`."""
    plan = Program.from_pir(str(project)).plan()
    before = plan.run().to_arrow().to_pylist()
    plan.explain("bel", "POL1", depth=-1)
    after = plan.run().to_arrow().to_pylist()
    assert before == after
