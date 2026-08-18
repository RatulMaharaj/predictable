"""The `E12xx` catalogue (`02-dsl.md` §10), with worked, LLM-actionable messages.

Each function here builds one diagnostic and raises it. The messages follow the rule of §10.1:
**state what is wrong, why the rule exists, and the exact replacement text**. They are written to
be acted on by an agent that has no other context than the message — so every one of them names
the construct that replaces the refused one, and where a mechanical fix exists it is attached as
a :class:`~predictable.diagnostics.Suggestion` with a byte-range edit.

Only the codes the *tracer* can raise live here. Declaration-time codes (`E11xx`, `E13xx`,
`E14xx`, `E15xx`, `E16xx`) belong to the declaration layer, with the two exceptions that fire on
a traced value's operators and therefore have to be raised from the proxy: `E1302` and `E1402`.
"""

from __future__ import annotations

from .diagnostics import Diagnostic, DslError, Edit, Suggestion, capture_span

__all__ = [
    "raise_e1201_truth_test",
    "raise_e1202_self_reference",
    "raise_e1203_non_constant_lag",
    "raise_e1204_forward_reference",
    "raise_e1205_iteration",
    "raise_e1206_non_builtin",
    "raise_e1207_stage2_in_expr",
    "raise_e1302_modelpoint_object",
    "raise_e1402_basis_conversion",
]


def _raise(diag: Diagnostic) -> None:
    raise DslError(diag)


# --------------------------------------------------------------------------------------
# E1201 — truth-testing a traced value
# --------------------------------------------------------------------------------------


def raise_e1201_truth_test(name: str, shape: str, construct: str = "if") -> None:
    """Python ``if`` / ``and`` / ``or`` / ``not`` / ``bool()`` applied to a model value."""
    span = capture_span(label=f"`{name}` is a {shape} component, not a Python bool", needle=name)
    replacement = {
        "and": "all_(a, b)",
        "or": "any_(a, b)",
        "not": "not_(x)",
    }.get(construct)
    if construct in ("and", "or", "not"):
        headline = f"`{construct}` cannot be used on a model value"
        why = (
            f"Python's `{construct}` is defined in terms of truthiness, so it cannot be\n"
            "overloaded — it would have to pick a branch at trace time, and there is no single\n"
            f"value here to pick from: `{name}` is a different value for every modelpoint and\n"
            "every t."
        )
        fix = (
            f"Use the value-level connective, which compiles to the IR's infix `{construct}`\n"
            f"and evaluates per period:\n\n    {replacement}"
        )
    else:
        headline = f"`{construct}` cannot be used on a model value"
        why = (
            "A component body is traced once at build time, so there is no single value here to\n"
            f"branch on — `{name}` is a different value for every modelpoint and every t."
        )
        fix = (
            "Use the value conditional `when(cond, then, otherwise)`, which compiles to the IR's\n"
            "`if ... then ... else ...` and evaluates per period:\n\n"
            f"    return when({name}, <then>, <otherwise>)"
        )
    _raise(
        Diagnostic(
            code="E1201",
            message=headline,
            spans=(span,),
            notes=(why,),
            suggestions=(
                Suggestion(
                    message=fix,
                    applicability="has_placeholders",
                ),
                Suggestion(
                    message=(
                        "Note `when` evaluates both branches; it is not a guard. To avoid a\n"
                        "division by zero, guard the denominator instead:\n"
                        "    when(n > 0, x / max_(n, 1.0), 0.0)"
                    ),
                    applicability="unspecified",
                ),
            ),
        )
    )


# --------------------------------------------------------------------------------------
# E1202 — unlagged self-reference
# --------------------------------------------------------------------------------------


def raise_e1202_self_reference(name: str) -> None:
    span = capture_span(label=f"this is {name}[t], inside {name}", needle=name)
    _raise(
        Diagnostic(
            code="E1202",
            message=f"`{name}` reads itself with no time lag",
            spans=(span,),
            notes=(
                "A component cannot depend on its own value in the same period — there is no\n"
                "order in which to compute it. Recursion across periods is fine and is how\n"
                "survivorship is written.",
            ),
            suggestions=(
                Suggestion(
                    message=f"lag the self-reference by one period:\n\n    {name}[t-1]",
                    edits=(
                        Edit(
                            file=span.file,
                            start=span.start,
                            end=span.end,
                            replacement=f"{name}[t-1]",
                        ),
                    ),
                ),
                Suggestion(
                    message=(
                        f"You will also need a starting value, since {name}[-1] does not exist:\n\n"
                        "    @series(timing=START, init=1.0)"
                    ),
                    applicability="has_placeholders",
                ),
            ),
        )
    )


# --------------------------------------------------------------------------------------
# E1203 — non-constant lag index
# --------------------------------------------------------------------------------------


def raise_e1203_non_constant_lag(name: str, offender: str) -> None:
    span = capture_span(label="the lag must be an integer known at build time", needle=name)
    _raise(
        Diagnostic(
            code="E1203",
            message=f"the lag index of `{name}` is not a build-time constant",
            spans=(span,),
            notes=(
                f"`{offender}` is a model value, so the lag would differ per modelpoint and per\n"
                "t. The IR's `Lag(name, k)` carries a fixed `k` because the planner sizes the\n"
                "retention ring from it before any data is read (`01-ir.md` §2.7, `03-engine.md`\n"
                "§3.4). A data-dependent lag has no ring size.\n\n"
                "A plain Python `int` is fine — `LAG = 3; x[t-LAG]` is constant-folded at trace\n"
                "time and appears in the IR as `x[t-3]`.",
            ),
            suggestions=(
                Suggestion(
                    message=(
                        "If the offset really varies with the data, model the choice as a value:\n\n"
                        f"    when(cond, {name}[t-1], {name}[t-2])"
                    ),
                    applicability="has_placeholders",
                ),
            ),
        )
    )


# --------------------------------------------------------------------------------------
# E1204 — forward time reference
# --------------------------------------------------------------------------------------


def raise_e1204_forward_reference(name: str, offset_text: str) -> None:
    span = capture_span(label="a projection is a single forward pass", needle=name)
    _raise(
        Diagnostic(
            code="E1204",
            message=f"`{name}[{offset_text}]` reads the future",
            spans=(span,),
            notes=(
                "Values are produced one period at a time, in increasing t, so at time t the\n"
                "value at t+1 does not exist yet. Backward-looking recursion is expressible;\n"
                "forward-looking is not (`01-ir.md` §2.7).\n\n"
                "The legal backward channel is a stage-2 aggregate over the completed series,\n"
                "read from `init` — that is how a prospective reserve seeds a retrospective\n"
                "roll-forward (`01-ir.md` §8.2).",
            ),
            suggestions=(
                Suggestion(
                    message=(
                        "Reduce the completed series in a @per_mp component and seed the\n"
                        "recursion with it:\n\n"
                        "    @per_mp()\n"
                        f"    def {name}_total({name}: Money, disc_factor: Factor) -> Money:\n"
                        f"        return npv({name}, disc_factor)\n\n"
                        f"    @series(timing=POINT, init={name}_total)"
                    ),
                    applicability="has_placeholders",
                ),
            ),
        )
    )


# --------------------------------------------------------------------------------------
# E1205 — loop / comprehension / iteration over a traced value
# --------------------------------------------------------------------------------------


def raise_e1205_iteration(name: str, operation: str = "iterate over") -> None:
    span = capture_span(label=f"`{name}` is one value per modelpoint per t, not a sequence", needle=name)
    _raise(
        Diagnostic(
            code="E1205",
            message=f"cannot {operation} a model value",
            spans=(span,),
            notes=(
                f"`{name}` is a symbolic value during tracing, not the projected series — the\n"
                "series does not exist until the engine runs, and the body is traced exactly\n"
                "once. So there is nothing to iterate, take the length of, or use as a dict key.\n\n"
                "Reductions over t are aggregates, and they run in stage 2 after the projection\n"
                "completes (`01-ir.md` §8.2).",
            ),
            suggestions=(
                Suggestion(
                    message=(
                        "To reduce over t, use an aggregate in a @per_mp body:\n\n"
                        f"    sum_({name})        # or npv({name}, disc_factor), last({name}),\n"
                        f"                        # max_over({name}), count_while(cond)\n\n"
                        f"To read one period, index it: {name}[0], {name}[t-1]."
                    ),
                    applicability="has_placeholders",
                ),
                Suggestion(
                    message=(
                        "A Python `for` loop that *declares* components is fine — that is code\n"
                        "generation, not computation (`02-dsl.md` §7.3). Only looping over a\n"
                        "traced value is refused."
                    ),
                    applicability="unspecified",
                ),
            ),
        )
    )


# --------------------------------------------------------------------------------------
# E1206 — call to a non-builtin
# --------------------------------------------------------------------------------------


def raise_e1206_non_builtin(name: str, called: str, suggestion: str | None = None) -> None:
    span = capture_span(label="not a builtin of IR 1.0", needle=name)
    notes = [
        f"`{called}` would have to run per modelpoint and per t, and no arbitrary Python\n"
        "crosses the engine boundary: the IR has no user-defined functions, so there is\n"
        "nothing to lower this to (`01-ir.md` §12, `02-dsl.md` §13).\n\n"
        "The builtin set is closed and is listed in `predictable.fn`.",
    ]
    fixes = []
    if suggestion:
        fixes.append(
            Suggestion(
                message=f"use the builtin instead:\n\n    {suggestion}",
                edits=(
                    Edit(
                        file=span.file, start=span.start, end=span.end, replacement=suggestion
                    ),
                ),
            )
        )
    fixes.append(
        Suggestion(
            message=(
                "If this is a helper you wrote, it can still generate *declarations* — a Python\n"
                "function that returns a traced expression built from builtins is inlined by\n"
                "the tracer. What it cannot do is compute on values itself."
            ),
            applicability="unspecified",
        )
    )
    _raise(
        Diagnostic(
            code="E1206",
            message=f"`{called}` is not a builtin and cannot be called on a model value",
            spans=(span,),
            notes=tuple(notes),
            suggestions=tuple(fixes),
        )
    )


# --------------------------------------------------------------------------------------
# E1207 — stage-2 value read in `expr`
# --------------------------------------------------------------------------------------


def raise_e1207_stage2_in_expr(name: str) -> None:
    span = capture_span(label="computed after the projection completes", needle=name)
    _raise(
        Diagnostic(
            code="E1207",
            message=f"`{name}` is a stage-2 value and cannot be read at time t",
            spans=(span,),
            notes=(
                f"`{name}` reduces the whole projection (it contains an aggregate), so it is only\n"
                "known once every period has been computed. Reading it inside `expr` would make\n"
                "period 0 depend on period T.\n\n"
                "Seeding a recursion with it is legal and is the single backward channel the IR\n"
                "allows: `init` is evaluated in stage 2 (`01-ir.md` §8.2).",
            ),
            suggestions=(
                Suggestion(
                    message=(
                        f"Move the reference into `init`:\n\n"
                        f"    @series(timing=POINT, init={name})\n\n"
                        f"and drop `{name}` from the body."
                    ),
                    applicability="maybe_incorrect",
                ),
            ),
        )
    )


# --------------------------------------------------------------------------------------
# E1302 — modelpoint object accessed as a value
# --------------------------------------------------------------------------------------


def raise_e1302_modelpoint_object(obj: str, attribute: str) -> None:
    span = capture_span(label="the modelpoint is a schema, not a record", needle=attribute)
    _raise(
        Diagnostic(
            code="E1302",
            message=f"`{obj}.{attribute}` — a modelpoint is not a value",
            spans=(span,),
            notes=(
                "A ModelPoint subclass is only a schema. It is never instantiated and no\n"
                "component receives one: components take individual fields as parameters, so\n"
                "the dependency set is visible in the signature (`02-dsl.md` §2.2, §3).",
            ),
            suggestions=(
                Suggestion(
                    message=(
                        f"add `{attribute}` to the parameter list and use it directly:\n\n"
                        f"    def <component>(..., {attribute}: <Unit>) -> ...:\n"
                        f"        return ... {attribute} ..."
                    ),
                    applicability="has_placeholders",
                ),
            ),
        )
    )


# --------------------------------------------------------------------------------------
# E1402 — arithmetic basis conversion on a Rate
# --------------------------------------------------------------------------------------


def raise_e1402_basis_conversion(name: str, basis: str, divisor: int | float) -> None:
    span = capture_span(label=f"{name} is Rate.{basis}", needle=name)
    periodic = {12: "to_monthly", 1: "to_annual"}.get(int(divisor) if divisor == int(divisor) else 0)
    fixes = []
    if periodic:
        fixes.append(
            Suggestion(
                message=(
                    "If you want the compounding conversion (almost always the case in a\n"
                    f"valuation):\n\n    {periodic}({name})"
                ),
                edits=(
                    Edit(
                        file=span.file,
                        start=span.start,
                        end=span.end,
                        replacement=f"{periodic}({name})",
                    ),
                ),
            )
        )
    fixes.append(
        Suggestion(
            message=(
                "If you genuinely want simple division — e.g. reproducing a Prophet variable\n"
                "that does `/12` and which you are matching to the penny — say so explicitly:\n\n"
                f"    nominal_to_periodic({name}, {divisor:g})\n\n"
                "The explicit form appears in the IR diff, so a reviewer sees the choice was made."
            ),
            applicability="maybe_incorrect",
        )
    )
    _raise(
        Diagnostic(
            code="E1402",
            message=f"a rate on an {basis} basis cannot be divided to change basis",
            spans=(span,),
            notes=(
                f"Dividing an {basis} rate by {divisor:g} gives a nominal rate, not the equivalent\n"
                f"effective rate for the shorter period, and understates discounting by roughly\n"
                f"(1+i)^(1/{divisor:g}) - 1 - i/{divisor:g}.",
            ),
            suggestions=tuple(fixes),
        )
    )


# --------------------------------------------------------------------------------------
# E1201 — a body that branched instead of returning a value
# --------------------------------------------------------------------------------------


def raise_e1201_no_return(component: str) -> None:
    """A traced body returned ``None`` — almost always a Python ``if`` that took no branch."""
    span = capture_span(label="traced once, and returned nothing", needle=component)
    _raise(
        Diagnostic(
            code="E1201",
            message=f"`{component}` did not return a model value",
            spans=(span,),
            notes=(
                "A component body is traced exactly once and must return the expression that\n"
                "defines it. A body that returns nothing has almost always used a Python `if`\n"
                "statement to choose between two returns — but the trace takes one path only,\n"
                "and the IR has no control flow to record it in (`02-dsl.md` §2.4).",
            ),
            suggestions=(
                Suggestion(
                    message=(
                        "Collapse the branches into a single value conditional:\n\n"
                        "    return when(<cond>, <then>, <otherwise>)"
                    ),
                    applicability="has_placeholders",
                ),
            ),
        )
    )
