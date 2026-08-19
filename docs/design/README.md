# predictable v2 — design documents

These are the normative specs for the v2 rebuild: an open-source (MIT)
actuarial modelling framework — a Rust projection engine executing a
serializable, git-diffable computation-graph IR, with a Python DSL as the
authoring layer.

Read them in order. Each one owns a layer of the stack, and each layer talks to
its neighbours only through the contract written down here.

```
  Python DSL  ──compiles to──▶  IR  ──executed by──▶  Rust engine
   (02-dsl)                  (01-ir)                  (03-engine)
        ▲                        │                         │
        │                        ▼                         ▼
   Prophet import ◀── verification & diffing (04-verify) ── results
                                 │
                                 ▼
                        visualization (05-viz)
```

## Index

| Doc | Owns | Answers |
| --- | --- | --- |
| [00-backlog.md](00-backlog.md) | The build backlog | What are the concrete tasks across all five specs, what depends on what, which phase does each belong to, and which can be built in parallel by separate agents? |
| [01-ir.md](01-ir.md) | The intermediate representation | What is the on-disk node/graph schema? How are types, time axes, table lookups and modelpoint fields represented? What are the versioning and diff-stability rules that keep the IR reviewable in a git PR? |
| [02-dsl.md](02-dsl.md) | The Python authoring layer | What does a model look like as source? How do `Model`, components, indicators and table references compile down to IR nodes — with no arbitrary Python lambdas crossing the engine boundary? |
| [03-engine.md](03-engine.md) | The Rust projection engine | How is the `(modelpoint x period)` grid evaluated? Scheduling, memory layout, parallelism, the PyO3/maturin wheel, and the later CLI and WASM targets. |
| [04-verify.md](04-verify.md) | The verification loop — **the product** | Definition-time validation with Rust/Elm-grade, LLM-actionable diagnostics; `explain()` provenance traces; Prophet `.rpt` structured run-diff; the AI-migration workflow this all serves. |
| [05-viz.md](05-viz.md) | Built-in visualization | The dependency-graph explorer and results views (waterfalls, run diffs, drill-down) behind `model.show()` / `results.show()`, served locally and WASM-capable. |

## Conventions used across these docs

- **Normative language.** MUST / SHOULD / MAY carry their usual RFC 2119 force.
  Anything else is commentary.
- **Schemas are real.** Every type shown is the type intended for
  implementation, not an illustration. If a doc and the code disagree, that is a
  bug in one of them — say which.
- **Prophet is the reference vocabulary.** Variables, indicators, MPF model
  point files, `.fac` tables and `.rpt` results are named as Prophet names them,
  because the first users are Prophet-shop actuaries migrating existing models.
- **Every boundary is serializable.** If it cannot be written to a file, hashed,
  and diffed, it does not belong in the IR.

## Status

Skeleton merged; documents authored per the roadmap. See
[`ROADMAP.md`](https://github.com/RatulMaharaj/predictable/blob/main/ROADMAP.md) for phasing, and note that
`docs/` also still contains the v0 MkDocs site (`index.md`, `tutorial.md`,
`reference.md`, …), which is untouched by the v2 work.
