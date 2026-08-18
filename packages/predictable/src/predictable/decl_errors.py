"""The declaration-layer catalogue: `E11xx`, `E13xx`, `E14xx`, `E15xx`, `E16xx`, `W11xx`.

:mod:`predictable.errors` owns the codes the *tracer* raises while a body runs. These are the ones
raised around a body — at the signature, at the schema, at the library boundary — and they follow
the same rule (`02-dsl.md` §10.1): **say what is wrong, why the rule exists, and the exact
replacement text**, with a byte-range :class:`~predictable.diagnostics.Edit` wherever the fix is
mechanical.

Spans here are computed from a declaration's own source line rather than from the live call stack,
because by the time these fire the stack is inside ``build()`` and the interesting line is the
``def`` the user wrote.
"""

from __future__ import annotations

import linecache
import os
from typing import Any, Callable, Iterable, Sequence

from .diagnostics import Diagnostic, DslError, Edit, LineCol, Span, Suggestion

__all__ = [
    "span_at",
    "span_of",
    "raise_e1101_missing_return_annotation",
    "raise_e1102_missing_timing",
    "raise_e1103_unknown_parameter",
    "raise_e1103_free_variable",
    "raise_e1104_annotation_conflict",
    "raise_e1105_ambiguous_name",
    "raise_e1106_table_signature",
    "raise_e1301_key_fields",
    "raise_e1401_timeline_redefinition",
    "raise_e1501_shadows_inherited",
    "raise_e1502_override_signature",
    "raise_e1203_unimplemented_abstract",
    "raise_e1504_final_override",
    "raise_e1601_output_manifest",
    "warn_w1101_unused",
]


def _raise(diag: Diagnostic) -> None:
    raise DslError(diag)


# --------------------------------------------------------------------------------------
# spans
# --------------------------------------------------------------------------------------


def span_at(
    file: str,
    line: int,
    *,
    needle: str | None = None,
    label: str | None = None,
    primary: bool = True,
) -> Span:
    """A span on ``file:line``, narrowed to ``needle`` when that token is on the line."""
    text = linecache.getline(file, line).rstrip("\n")
    stripped = text.strip()
    col = text.index(stripped) if stripped else 0
    width = max(len(stripped), 1)
    if needle and needle in text:
        col = text.index(needle)
        width = len(needle)
    line_start = sum(len(linecache.getline(file, i)) for i in range(1, line))
    start = line_start + col
    return Span(
        file=os.path.relpath(file) if os.path.exists(file) else file,
        start=start,
        end=start + width,
        primary=primary,
        label=label,
        start_pos=LineCol(line, col + 1),
        text=text,
    )


def span_of(
    func: Callable[..., Any] | None,
    *,
    needle: str | None = None,
    label: str | None = None,
    primary: bool = True,
) -> Span:
    """The span of a declaration's ``def`` line."""
    if func is None or not hasattr(func, "__code__"):  # pragma: no cover - defensive
        return Span("<unknown>", 0, 0, primary, label, LineCol(0, 0), "")
    code = func.__code__
    return span_at(
        code.co_filename, code.co_firstlineno, needle=needle, label=label, primary=primary
    )


def _def_line(func: Callable[..., Any]) -> int:
    """The line the ``def`` itself is on, skipping the decorator lines above it."""
    code = func.__code__
    lineno = code.co_firstlineno
    for offset in range(0, 12):
        text = linecache.getline(code.co_filename, lineno + offset)
        if text.lstrip().startswith(("def ", "async def ", "class ")):
            return lineno + offset
    return lineno


def _signature_span(
    func: Callable[..., Any], needle: str | None, label: str | None, primary: bool = True
) -> Span:
    return span_at(
        func.__code__.co_filename, _def_line(func), needle=needle, label=label, primary=primary
    )


def _listing(names: Iterable[str], limit: int = 12) -> str:
    names = list(names)
    shown = ", ".join(names[:limit])
    return shown + (f", … ({len(names) - limit} more)" if len(names) > limit else "") or "(none)"


def _closest(name: str, candidates: Iterable[str]) -> str | None:
    """Levenshtein-nearest candidate within an edit distance of 2 — the `did you mean` fix."""
    best, best_d = None, 3
    for candidate in candidates:
        d = _levenshtein(name, candidate)
        if d < best_d:
            best, best_d = candidate, d
    return best


def _levenshtein(a: str, b: str) -> int:
    if a == b:
        return 0
    previous = list(range(len(b) + 1))
    for i, ca in enumerate(a, 1):
        current = [i]
        for j, cb in enumerate(b, 1):
            current.append(min(previous[j] + 1, current[j - 1] + 1, previous[j - 1] + (ca != cb)))
        previous = current
    return previous[-1]


# --------------------------------------------------------------------------------------
# E1101 — missing return annotation
# --------------------------------------------------------------------------------------


def raise_e1101_missing_return_annotation(name: str, func: Callable[..., Any]) -> None:
    span = _signature_span(func, name, "no return annotation, so no dtype and no unit")
    _raise(
        Diagnostic(
            code="E1101",
            message=f"`{name}` has no return annotation",
            spans=(span,),
            notes=(
                "The return annotation is where a component's dtype and unit come from; the DSL\n"
                "never infers a unit, because an inferred unit cannot catch the error it exists\n"
                "to catch — adding money to a probability (`01-ir.md` §2.4).",
            ),
            suggestions=(
                Suggestion(
                    message=(
                        "Annotate the return with the quantity this component is:\n\n"
                        "    Money | Prob | Count | Rate.annual | Years | Factor | Flag | Num\n\n"
                        f"    def {name}(...) -> Money:"
                    ),
                    applicability="has_placeholders",
                ),
            ),
        )
    )


# --------------------------------------------------------------------------------------
# E1102 — @series without timing
# --------------------------------------------------------------------------------------


def raise_e1102_missing_timing(name: str | None = None) -> None:
    from .diagnostics import capture_span

    span = capture_span(label="`timing=` is required on @series")
    _raise(
        Diagnostic(
            code="E1102",
            message="`@series` requires `timing=`",
            spans=(span,),
            notes=(
                "There is no default. An actuary who has not decided whether a cashflow is in\n"
                "advance or in arrears has not finished modelling it, and the IR consumes the tag\n"
                "to pick the discounting exponent in `npv` (`01-ir.md` §2.5).",
            ),
            suggestions=(
                Suggestion(
                    message=(
                        "Pick one of the four tags:\n\n"
                        "    START  in advance — realised at the beginning of period t (premiums)\n"
                        "    END    in arrears — realised at the end of period t (claims paid)\n"
                        "    MID    mid-period approximation (the usual claims convention)\n"
                        "    POINT  a stock, not a flow — a balance at an instant (reserve)"
                    ),
                    applicability="has_placeholders",
                ),
            ),
        )
    )


# --------------------------------------------------------------------------------------
# E1103 — unknown parameter / free-variable capture
# --------------------------------------------------------------------------------------


def raise_e1103_unknown_parameter(
    component: str,
    param: str,
    func: Callable[..., Any],
    *,
    components: Sequence[str] = (),
    fields: Sequence[str] = (),
    assumptions: Sequence[str] = (),
    tables: Sequence[str] = (),
    timeline: Sequence[str] = (),
) -> None:
    span = _signature_span(
        func,
        param,
        "not a component, modelpoint field, assumption, table, or timeline input",
    )
    everything = list(components) + list(fields) + list(assumptions) + list(tables) + list(timeline)
    near = _closest(param, everything)
    suggestions = []
    if near is not None:
        suggestions.append(
            Suggestion(
                message=f"there is a name `{near}` in scope; did you mean it?",
                edits=(Edit(span.file, span.start, span.end, near),),
            )
        )
    _raise(
        Diagnostic(
            code="E1103",
            message=f"unknown name `{param}` in the parameter list of `{component}`",
            spans=(span,),
            notes=(
                f"Components in scope: {_listing(components)}\n"
                f"Modelpoint fields: {_listing(fields)}\n"
                f"Assumptions: {_listing(assumptions)}\n"
                f"Tables: {_listing(tables)}\n"
                f"Timeline inputs: {_listing(timeline)}",
            ),
            suggestions=tuple(suggestions),
        )
    )


def raise_e1103_free_variable(component: str, name: str, func: Callable[..., Any]) -> None:
    span = _signature_span(func, "def", f"`{name}` is used in the body but is not a parameter")
    _raise(
        Diagnostic(
            code="E1103",
            message=f"`{component}` reads `{name}` as a free variable",
            spans=(span,),
            notes=(
                "A component's dependencies are exactly its parameters (`02-dsl.md` §2.2). Lexical\n"
                "capture would make the dependency set invisible in the signature, which is what\n"
                "the model explorer, `explain()` and the impact analysis all read.",
            ),
            suggestions=(
                Suggestion(
                    message=(
                        f"Add `{name}` to the parameter list:\n\n"
                        f"    def {component}(..., {name}: <Unit>) -> ...:"
                    ),
                    applicability="has_placeholders",
                ),
            ),
        )
    )


# --------------------------------------------------------------------------------------
# E1104 — annotation contradicts the declaration
# --------------------------------------------------------------------------------------


def raise_e1104_annotation_conflict(
    component: str,
    param: str,
    func: Callable[..., Any],
    *,
    annotated: tuple[str, str],
    declared: tuple[str, str],
    declared_at: Span | None = None,
) -> None:
    spans = [
        _signature_span(
            func, param, f"you annotated `{param}` as {annotated[1]} ({annotated[0]})"
        )
    ]
    if declared_at is not None:
        spans.append(
            Span(
                file=declared_at.file,
                start=declared_at.start,
                end=declared_at.end,
                primary=False,
                label=f"but `{param}` is declared as {declared[1]} ({declared[0]})",
                start_pos=declared_at.start_pos,
                text=declared_at.text,
            )
        )
    _raise(
        Diagnostic(
            code="E1104",
            message=f"parameter annotation contradicts the declaration of `{param}`",
            spans=tuple(spans),
            notes=(
                "Parameter annotations are optional — but when present they are checked, so that\n"
                "a misunderstanding about what a component *means* fails at the signature rather\n"
                "than producing a plausible-looking wrong number.",
            ),
            suggestions=(
                Suggestion(
                    message=(
                        f"Either fix the annotation to match the declaration "
                        f"({declared[1]}, {declared[0]}), or, if you meant a different quantity, "
                        f"you are reading the wrong component."
                    ),
                    applicability="maybe_incorrect",
                ),
            ),
        )
    )


# --------------------------------------------------------------------------------------
# E1105 — ambiguous name
# --------------------------------------------------------------------------------------


def raise_e1105_ambiguous_name(
    component: str, param: str, func: Callable[..., Any], where: Sequence[str]
) -> None:
    span = _signature_span(func, param, "resolves to more than one declaration")
    _raise(
        Diagnostic(
            code="E1105",
            message=f"`{param}` is ambiguous: {', '.join(where)}",
            spans=(span,),
            notes=(
                "Resolution walks one namespace at a time — components in this module, then\n"
                "imported components, then modelpoint fields, assumptions, timeline inputs and\n"
                "tables — and takes the first match. Two matches at the same level cannot be\n"
                "ordered, so the model would depend on import order.",
            ),
            suggestions=(
                Suggestion(
                    message=(
                        "Rename one of them. A qualified reference is deliberately not available:\n"
                        "unqualified names are globally unique within a product (`01-ir.md` §2.2)."
                    ),
                    applicability="has_placeholders",
                ),
            ),
        )
    )


# --------------------------------------------------------------------------------------
# E1106 — table used but not in the signature (or vice versa)
# --------------------------------------------------------------------------------------


def raise_e1106_table_signature(
    component: str, table: str, func: Callable[..., Any], *, unused: bool
) -> None:
    if unused:
        message = f"`{component}` takes table `{table}` but never looks it up"
        note = (
            "A table in the signature is a declared dependency: it appears in the graph, in the\n"
            "run manifest and in the digest set. One that is never called is a stale dependency\n"
            "and makes a table swap look riskier than it is."
        )
        fix = f"Remove `{table}={table}` from the parameter list."
    else:
        message = f"`{component}` looks up table `{table}`, which is not in its signature"
        note = (
            "Tables follow the same rule as components: dependencies are parameters\n"
            "(`02-dsl.md` §6). The `tbl=tbl` default-argument idiom is what makes a table swap a\n"
            "signature-level change rather than an invisible one."
        )
        fix = f"Add `{table}={table}` to the parameter list."
    span = _signature_span(func, table if unused else "def", "table dependency mismatch")
    _raise(
        Diagnostic(
            code="E1106",
            message=message,
            spans=(span,),
            notes=(note,),
            suggestions=(Suggestion(message=fix, applicability="machine_applicable"),),
        )
    )


# --------------------------------------------------------------------------------------
# E1301 — modelpoint key fields
# --------------------------------------------------------------------------------------


def raise_e1301_key_fields(schema: str, keys: Sequence[str], file: str, line: int) -> None:
    span = span_at(file, line, needle=schema, label=f"{len(keys)} key() fields, expected exactly 1")
    if keys:
        detail = "declared keys: " + ", ".join(keys)
        fix = "Keep exactly one; the others are ordinary fields."
    else:
        detail = "no field is marked `key()`"
        fix = (
            "Mark the identity column:\n\n"
            "    policy_number: str = key()"
        )
    _raise(
        Diagnostic(
            code="E1301",
            message=f"modelpoint schema `{schema}` must have exactly one `key()` field",
            spans=(span,),
            notes=(
                f"{detail}.\nThe key is the join column for results, `explain()` and every\n"
                "run-diff alignment (`01-ir.md` §8.4.1); without exactly one, two runs cannot be\n"
                "compared row by row.",
            ),
            suggestions=(Suggestion(message=fix, applicability="has_placeholders"),),
        )
    )


# --------------------------------------------------------------------------------------
# E1401 — redefinition of a timeline input
# --------------------------------------------------------------------------------------


def raise_e1401_timeline_redefinition(name: str, span: Span) -> None:
    _raise(
        Diagnostic(
            code="E1401",
            message=f"`{name}` is a timeline input; you cannot redefine it",
            spans=(span,),
            notes=(
                "The timeline generates t, policy_year, policy_month, period_start_date,\n"
                "period_end_date, year_frac, month_of_year and is_anniversary as `Input.Timeline`\n"
                "components. They are always available and never redeclared (`01-ir.md` §5).",
            ),
            suggestions=(
                Suggestion(
                    message=f"Give the derived quantity its own name, e.g. `{name}_capped`.",
                    applicability="has_placeholders",
                ),
            ),
        )
    )


# --------------------------------------------------------------------------------------
# E1501 / E1502 / E1504 / E1203 — the library layer
# --------------------------------------------------------------------------------------


def raise_e1501_shadows_inherited(name: str, span: Span, inherited_from: str) -> None:
    _raise(
        Diagnostic(
            code="E1501",
            message=f"`{name}` shadows an inherited component",
            spans=(span,),
            notes=(
                f"`{name}` is inherited via `extends({inherited_from})`. Silent shadowing of a\n"
                "library component is not allowed: a reader of this product cannot tell whether\n"
                "the library value or the local one is in force.",
            ),
            suggestions=(
                Suggestion(
                    message=(
                        "If you mean to replace it, say so — the decorator also checks that shape,\n"
                        "dtype, unit and timing still match the library's contract:\n\n"
                        f"    @override({inherited_from}.{name})\n"
                        "    @series(timing=...)\n"
                        f"    def {name}(...):"
                    ),
                    applicability="has_placeholders",
                ),
                Suggestion(
                    message=(
                        f"If you meant a different quantity, give it a different name (e.g.\n"
                        f"`{name}_select`) and reference the library one alongside it."
                    ),
                    applicability="unspecified",
                ),
            ),
        )
    )


def raise_e1502_override_signature(
    name: str, span: Span, differences: Sequence[tuple[str, str, str]]
) -> None:
    detail = "\n".join(
        f"    {field}: base is {base}, override is {new}" for field, base, new in differences
    )
    _raise(
        Diagnostic(
            code="E1502",
            message=f"`@override` of `{name}` changes its contract",
            spans=(span,),
            notes=(
                "An override replaces a value, not its meaning. Shape, dtype, unit and timing are\n"
                "the contract every other component in the library was written against:\n\n"
                f"{detail}",
            ),
            suggestions=(
                Suggestion(
                    message=(
                        "Match the base declaration. Changing the contract deliberately means the\n"
                        "base component was wrong — fix it there — or that you want a new name."
                    ),
                    applicability="maybe_incorrect",
                ),
            ),
        )
    )


def raise_e1504_final_override(name: str, span: Span, library: str) -> None:
    _raise(
        Diagnostic(
            code="E1504",
            message=f"`{name}` is `@final` and cannot be overridden",
            spans=(span,),
            notes=(
                f"`{library}.{name}` is marked `@final`, which is reserved for regulatory or\n"
                "group-standard components — the ones whose whole value is that every product\n"
                "computes them the same way.",
            ),
            suggestions=(
                Suggestion(
                    message=(
                        f"If this product genuinely needs a different quantity, declare it under a\n"
                        f"new name (e.g. `{name}_local`) and leave `{name}` in force, so the\n"
                        "deviation is visible in the diff."
                    ),
                    applicability="has_placeholders",
                ),
            ),
        )
    )


def raise_e1203_unimplemented_abstract(
    product_name: str, missing: Sequence[tuple[str, str, str]], span: Span
) -> None:
    """Undischarged `@abstract` (IR §13 Q10 — `E1203` at build, before any `.pir` is written)."""
    detail = "\n".join(
        f"    {signature}\n        {decorator}    {where}" for signature, decorator, where in missing
    )
    _raise(
        Diagnostic(
            code="E1203",
            message=(
                f"product `{product_name}` has {len(missing)} unimplemented abstract "
                f"component{'s' if len(missing) != 1 else ''}"
            ),
            spans=(span,),
            notes=(
                "Required, with the signature each override must match:\n\n"
                f"{detail}\n\n"
                "There is no `Kind::Abstract` in the IR (`01-ir.md` §13 Q10): an undischarged\n"
                "abstract fails the build and no `.pir` is written, so a well-formed module never\n"
                "contains a hole. (`02-dsl.md` §10 catalogues this case as `E1503`; the IR decision\n"
                "log fixes the emitted code at `E1203`.)",
            ),
            suggestions=(
                Suggestion(
                    message=(
                        "Add them to a module in this product, decorated with `@override(<lib>.<name>)`.\n"
                        "If this product genuinely has no such value, override it with a zero and say\n"
                        "why in the docstring — an explicit zero is auditable, a missing component is not."
                    ),
                    applicability="has_placeholders",
                ),
            ),
        )
    )


# --------------------------------------------------------------------------------------
# E1601 — product outputs manifest
# --------------------------------------------------------------------------------------


def raise_e1601_output_manifest(
    product_name: str, missing: Sequence[str], extra: Sequence[str], span: Span
) -> None:
    parts = []
    if missing:
        parts.append(
            "listed in `outputs=` but not marked `output=True`: " + ", ".join(sorted(missing))
        )
    if extra:
        parts.append(
            "marked `output=True` but absent from `outputs=`: " + ", ".join(sorted(extra))
        )
    _raise(
        Diagnostic(
            code="E1601",
            message=f"product `{product_name}` output list disagrees with the model",
            spans=(span,),
            notes=(
                "\n".join(parts) + "\n\n"
                "`output=True` on the component is the single source of emission truth\n"
                "(`01-ir.md` §13 Q2); `outputs=` is a checked manifest, so that a reviewer sees the\n"
                "promised result set in one readable place and adding an output shows up in two.",
            ),
            suggestions=(
                Suggestion(
                    message=(
                        "Make the two agree — either add the missing name to `outputs=` or drop\n"
                        "`output=True` from the component."
                    ),
                    applicability="maybe_incorrect",
                ),
            ),
        )
    )


# --------------------------------------------------------------------------------------
# W1101 — declared but never read
# --------------------------------------------------------------------------------------


def warn_w1101_unused(name: str, span: Span) -> Diagnostic:
    """A lint, returned rather than raised — warnings never stop a build."""
    return Diagnostic(
        code="W1101",
        severity="warning",
        message=f"`{name}` is never read and is not an output",
        spans=(span,),
        notes=(
            "Nothing depends on it and it is not emitted, so it costs a slot per modelpoint per\n"
            "period and appears in no result.",
        ),
        suggestions=(
            Suggestion(
                message=(
                    f"Mark it as a result if it is one — `@series(..., output=True)` — or delete it."
                ),
                applicability="maybe_incorrect",
            ),
        ),
    )
