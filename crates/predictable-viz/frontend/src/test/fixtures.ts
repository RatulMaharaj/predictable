// Test fixtures.
//
// `term_annual.graph.json` is **not hand-written**: it is the verbatim output of
//
//     predictable graph models/term_annual/build/{schema,product,model}.pir --json
//
// so every test below runs against the planner's own ordering, the planner's own
// depths and the real `model_digest` — which is the only way to catch the viz
// layer quietly re-deriving something the engine already decided.

import graph from "./fixtures/term_annual.graph.json";
import rundiff from "./fixtures/term_annual.rundiff.json";
import type {
  Capabilities,
  ComponentDoc,
  DataSource,
  GraphDoc,
  RunDiffDoc,
  SeriesQuery,
} from "../datasource";

export const TERM_ANNUAL = graph as unknown as GraphDoc;

/**
 * `term_annual.rundiff.json` is likewise **not hand-written**: it is the verbatim
 * output of
 *
 *     predictable diff run good/runs/base bad/runs/base \
 *         --model-a good/build --model-b bad/build --json
 *
 * over the two models of `docs/v2/verification-walkthrough.md` — the escalation
 * applied a year early. So Screen 2's types are held against the diff's own
 * writer rather than against a fixture someone typed to match them.
 */
export const TERM_ANNUAL_RUNDIFF = rundiff as unknown as RunDiffDoc;

/**
 * The same graph with Prophet provenance and a lint attached to two components.
 *
 * The `.pir` text front end does not yet carry `[component.meta]` (see the docs
 * page: `meta.source` reaches `GraphDoc` only once `predictable-syntax` lowers
 * it), so the migration case — "type `NUM_POLS_IF`, land on `num_pols_if`" — is
 * exercised against a document shaped exactly as §2.1 specifies it.
 */
export function withProphet(doc: GraphDoc = TERM_ANNUAL): GraphDoc {
  const map: Record<string, { library: string; variable: string; file: string }> = {
    "model.num_pols_if": {
      library: "TERM_UK",
      variable: "NUM_POLS_IF",
      file: "TERM.VAR:42",
    },
    "model.death_claims": {
      library: "TERM_UK",
      variable: "DTH_OUTGO",
      file: "TERM.VAR:88",
    },
    "model.reserve": {
      library: "TERM_UK",
      variable: "BEL_TOT",
      file: "TERM.VAR:118",
    },
  };
  return {
    ...doc,
    nodes: doc.nodes.map((node) => {
      const source = map[node.id];
      const lints =
        node.id === "model.expense_scale"
          ? [
              {
                code: "W0104",
                severity: "warn",
                message: "constant across modelpoints and time",
              },
            ]
          : undefined;
      return {
        ...node,
        ...(source ? { source: { system: "prophet", ...source } } : {}),
        ...(lints ? { lints } : {}),
      };
    }),
  };
}

/** A `DataSource` over a static document — the §1.1 contract, no server. */
export class StubDataSource implements DataSource {
  constructor(
    private readonly doc: GraphDoc,
    private readonly caps: Capabilities = {
      explain: false,
      recompute: false,
      sensitivity: false,
    },
  ) {}

  capabilities(): Promise<Capabilities> {
    return Promise.resolve(this.caps);
  }
  manifest(): Promise<unknown> {
    return Promise.resolve({});
  }
  graph(): Promise<GraphDoc> {
    return Promise.resolve(this.doc);
  }
  component(name: string): Promise<ComponentDoc> {
    const node = this.doc.nodes.find((n) => n.id === name || n.name === name);
    if (!node) return Promise.reject(new Error(`unknown component ${name}`));
    const upstream = Array.from(
      new Set(this.doc.edges.filter((e) => e.to === node.id).map((e) => e.from)),
    );
    const downstream = Array.from(
      new Set(this.doc.edges.filter((e) => e.from === node.id).map((e) => e.to)),
    );
    return Promise.resolve({ node, upstream, downstream });
  }
  series(_q: SeriesQuery): Promise<never> {
    return Promise.reject(new Error("no run loaded"));
  }
  explain(): Promise<never> {
    return Promise.reject(new Error("no run loaded"));
  }
  diff(): Promise<never> {
    return Promise.reject(new Error("no run loaded"));
  }
}

/** An in-memory `LayoutStore`, so the cache tests do not touch `localStorage`. */
export class MemoryStore {
  readonly map = new Map<string, string>();
  getItem(key: string): string | null {
    return this.map.get(key) ?? null;
  }
  setItem(key: string, value: string): void {
    this.map.set(key, value);
  }
  removeItem(key: string): void {
    this.map.delete(key);
  }
}
