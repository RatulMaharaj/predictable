"""The IR expression tree: serde parity, canonical text, float form."""

from __future__ import annotations

import json

import pytest

from predictable.ir import (
    Agg,
    At,
    Binary,
    Call,
    If,
    Lag,
    Lit,
    Lookup,
    Ref,
    Unary,
    format_f64,
    from_json,
    lit_of,
    walk,
)


def test_json_is_internally_tagged_on_node_like_serde():
    node = Binary("mul", Ref("sum_assured"), Lag("deaths", 1))
    assert node.to_json() == {
        "node": "Binary",
        "op": "mul",
        "lhs": {"node": "Ref", "name": "sum_assured"},
        "rhs": {"node": "Lag", "name": "deaths", "k": 1},
    }


def test_call_uses_the_fn_key_serde_renames_to():
    assert Call("npv", (Ref("x"), Ref("v"))).to_json()["fn"] == "npv"


def test_if_uses_the_else_key_serde_renames_to():
    doc = If(Ref("c"), Lit("f64", 1.0), Lit("f64", 0.0)).to_json()
    assert doc["node"] == "If" and "else" in doc and "otherwise" not in doc


def test_agg_omits_an_absent_predicate_like_skip_serializing_if():
    assert "pred" not in Agg("sum", Ref("x")).to_json()
    assert Agg("sum", Ref("x"), Ref("c")).to_json()["pred"] == {"node": "Ref", "name": "c"}


@pytest.mark.parametrize(
    "node",
    [
        Lit("f64", 1.05),
        Ref("x"),
        Lag("x", 2),
        At("x", 0),
        Unary("neg", Ref("x")),
        Binary("add", Ref("x"), Lit("i64", 1)),
        If(Ref("c"), Ref("a"), Ref("b")),
        Call("retime", (Ref("x"), Lit("str", "mid"))),
        Lookup("sa8990", (Ref("age"), Ref("gender"))),
        Agg("npv", Ref("x"), Ref("c")),
    ],
)
def test_every_node_round_trips_through_json(node):
    assert from_json(json.loads(json.dumps(node.to_json()))) == node


def test_walk_is_preorder_and_complete():
    tree = Binary("add", Ref("a"), Binary("mul", Ref("b"), Ref("c")))
    assert [type(n).__name__ for n in walk(tree)] == [
        "Binary",
        "Ref",
        "Binary",
        "Ref",
        "Ref",
    ]


# -- canonical text ----------------------------------------------------------------------


@pytest.mark.parametrize(
    "node,text",
    [
        (Binary("mul", Ref("a"), Ref("b")), "a * b"),
        # `*` binds tighter than `+`, so no parentheses are invented.
        (Binary("add", Binary("mul", Ref("a"), Ref("b")), Ref("c")), "a * b + c"),
        # ... but they are kept where the tree needs them.
        (Binary("mul", Binary("add", Ref("a"), Ref("b")), Ref("c")), "(a + b) * c"),
        # left-associative operators need parentheses on the right at equal precedence
        (Binary("sub", Ref("a"), Binary("sub", Ref("b"), Ref("c"))), "a - (b - c)"),
        (Binary("sub", Binary("sub", Ref("a"), Ref("b")), Ref("c")), "a - b - c"),
        # `^` is right-associative: the tight side is the left one
        (Binary("pow", Ref("a"), Binary("pow", Ref("b"), Ref("c"))), "a ^ b ^ c"),
        (Binary("pow", Binary("pow", Ref("a"), Ref("b")), Ref("c")), "(a ^ b) ^ c"),
        (Unary("neg", Binary("add", Ref("a"), Ref("b"))), "-(a + b)"),
        (Unary("not", Ref("flag")), "not flag"),
        (Lag("num_pols_if", 1), "num_pols_if[t-1]"),
        (At("premium_rate", 0), "premium_rate[0]"),
        (If(Ref("c"), Lit("f64", 1.0), Lit("f64", 0.0)), "if c then 1.0 else 0.0"),
        (Lookup("sa8990", (Ref("age"), Ref("gender"))), "sa8990@(age, gender)"),
        (Call("npv", (Ref("x"), Ref("v"))), "npv(x, v)"),
        # the retime timing tag prints bare, not quoted
        (Call("retime", (Ref("x"), Lit("str", "mid"))), "retime(x, mid)"),
        (Lit("bool", True), "true"),
        (Lit("i64", 3), "3"),
        (Lit("str", 'a "b"'), '"a \\"b\\""'),
        (Agg("sum", Ref("x")), "sum(x)"),
    ],
)
def test_canonical_expression_text(node, text):
    assert node.to_pir() == text


def test_comparisons_never_chain():
    node = Binary("lt", Binary("lt", Ref("a"), Ref("b")), Ref("c"))
    assert node.to_pir() == "(a < b) < c"


@pytest.mark.parametrize(
    "value,text",
    # the four values `predictable-fmt` pins in its own tests, plus the shortest-form cases
    [(1.05, "1.05"), (0.10000000000000001, "0.1"), (0.0, "0.0"), (1e-8, "1e-8"),
     (1000000.0, "1000000.0"), (45.0, "45.0"), (-0.0, "-0.0"), (1.5e-9, "1.5e-9")],
)
def test_float_form_matches_ryu(value, text):
    assert format_f64(value) == text


def test_a_float_never_prints_as_an_integer():
    for value in (1.0, 45.0, -3.0, 1e15, 1e16):
        printed = format_f64(value)
        assert "." in printed or "e" in printed


def test_lit_of_gives_the_dtype_the_ir_gives_it():
    assert lit_of(True) == Lit("bool", True)
    assert lit_of(3) == Lit("i64", 3)
    assert lit_of(3.0) == Lit("f64", 3.0)
    assert lit_of("mid") == Lit("str", "mid")
    with pytest.raises(TypeError):
        lit_of(object())  # type: ignore[arg-type]
