// Real data for Screen 2's tests.
//
// None of this is hand-written. It is the verification walkthrough
// (`docs/v2/verification-walkthrough.md`) replayed against the release binary:
// `models/term_annual` copied to `good/` and `bad/`, `premium_escalation` set
// live at 0.03 on both sides, and the escalation applied a year early in
// `bad/build/model.pir` (`init = "annual_premium * (1 + premium_escalation)"`).
// Both sides were run with the shipped `run.pir` (`emit = "outputs"`), which
// reproduces the walkthrough's manifest digests exactly:
//
//   good sha256:eb888afd…  bad sha256:14fab3b6…
//
//   series-{a,b}.json   `results.parquet`, pivoted to the `/api/series` wire
//                       shape: `modelpoint`, `t`, one column per component.
//   modelpoints.json    `models/term_annual/data/modelpoints.csv`, the numeric
//                       declared fields, as `/api/series` would serve them.
//   trace-{a,b}.json    `predictable explain … --component premium_income
//                       --mp TA00001 --t 0 --json`, verbatim.
//
//   diff.json           `predictable diff run good/runs/base bad/runs/base
//                       --model-a good/build --model-b bad/build --json`,
//                       verbatim — the same document the terminal render in the
//                       walkthrough is printed from.
//
// The graph is the shared `test/fixtures/term_annual.graph.json`, which is the
// planner's own `predictable graph --json` for the same model.

import { tableFromArrays, type Table } from "apache-arrow";
import type {
  Capabilities,
  ComponentDoc,
  DataSource,
  ExplainQuery,
  GraphDoc,
  RunDiffDoc,
  SeriesQuery,
} from "../../datasource";
import { ApiError } from "../../datasource";
import { TERM_ANNUAL } from "../../test/fixtures";
import diff from "./diff.json";
import seriesA from "./series-a.json";
import seriesB from "./series-b.json";
import modelpoints from "./modelpoints.json";
import traceA from "./trace-a.json";
import traceB from "./trace-b.json";

interface SeriesFixture {
  run?: string;
  modelpoint: string[];
  t?: number[];
  components: string[];
  columns: Record<string, (number | null)[]>;
}

export const WALKTHROUGH_DIFF: RunDiffDoc = diff as unknown as RunDiffDoc;
export const WALKTHROUGH_GRAPH: GraphDoc = TERM_ANNUAL;
export const TRACE_A = traceA as unknown;
export const TRACE_B = traceB as unknown;

type Cube = Map<string, Map<string, Map<number, number | null>>>;

function cube(fixture: SeriesFixture, constantInT: boolean): Cube {
  const out: Cube = new Map();
  for (const component of fixture.components) {
    const byMp = new Map<string, Map<number, number | null>>();
    const values = fixture.columns[component];
    fixture.modelpoint.forEach((mp, i) => {
      let row = byMp.get(mp);
      if (!row) {
        row = new Map();
        byMp.set(mp, row);
      }
      row.set(constantInT ? -1 : (fixture.t?.[i] ?? 0), values[i]);
    });
    out.set(component, byMp);
  }
  return out;
}

/**
 * A `DataSource` over the walkthrough's real artefacts.
 *
 * It answers `/api/series` with the same schema `source.rs` fixes —
 * `modelpoint: Utf8`, `t`, then one `Float64` per requested component in
 * request order — so the screen is exercised against the wire shape it will
 * meet in the server, not against a convenience object.
 */
export class WalkthroughSource implements DataSource {
  private readonly a: Cube;
  private readonly b: Cube;
  private readonly fields: Cube;
  private readonly periods: number[];
  private readonly modelpoints: string[];
  readonly calls: { series: SeriesQuery[]; explain: ExplainQuery[] } = { series: [], explain: [] };

  constructor(
    private readonly doc: RunDiffDoc = WALKTHROUGH_DIFF,
    private readonly graphDoc: GraphDoc | null = WALKTHROUGH_GRAPH,
    private readonly caps: Capabilities = { explain: true, recompute: false, sensitivity: false },
  ) {
    const fa = seriesA as SeriesFixture;
    const fb = seriesB as SeriesFixture;
    this.a = cube(fa, false);
    this.b = cube(fb, false);
    this.fields = cube(modelpoints as SeriesFixture, true);
    this.periods = Array.from(new Set(fa.t ?? [])).sort((x, y) => x - y);
    this.modelpoints = Array.from(new Set(fa.modelpoint)).sort();
  }

  capabilities(): Promise<Capabilities> {
    return Promise.resolve(this.caps);
  }
  manifest(): Promise<unknown> {
    return Promise.resolve({ a: this.doc.a, b: this.doc.b });
  }
  graph(): Promise<GraphDoc> {
    return this.graphDoc
      ? Promise.resolve(this.graphDoc)
      : Promise.reject(new ApiError(501, "unsupported", "no graph in this pack"));
  }
  component(name: string): Promise<ComponentDoc> {
    const node = this.graphDoc?.nodes.find((n) => n.id === name || n.name === name);
    if (!node) return Promise.reject(new Error(`unknown component ${name}`));
    return Promise.resolve({ node, upstream: [], downstream: [] });
  }
  diff(a: string, b: string): Promise<RunDiffDoc> {
    if (a !== this.doc.a.run || b !== this.doc.b.run) {
      return Promise.reject(new ApiError(404, "unknown_run", `no diff for ${a} vs ${b}`));
    }
    return Promise.resolve(this.doc);
  }

  series(q: SeriesQuery): Promise<Table> {
    this.calls.series.push(q);
    const side = q.run === this.doc.b.run ? this.b : this.a;
    const mps = q.modelpoints?.length ? q.modelpoints : this.modelpoints;
    const fieldsOnly = q.components.every((c) => this.fields.has(c));
    const periods = fieldsOnly
      ? [0]
      : this.periods.filter((t) => t >= (q.tFrom ?? -Infinity) && t <= (q.tTo ?? Infinity));

    const mpColumn: string[] = [];
    const tColumn: number[] = [];
    const columns: Record<string, number[]> = {};
    for (const component of q.components) columns[component] = [];
    for (const mp of mps) {
      for (const t of periods) {
        mpColumn.push(mp);
        tColumn.push(t);
        for (const component of q.components) {
          const source = this.fields.get(component) ?? side.get(component);
          const value = source?.get(mp)?.get(this.fields.has(component) ? -1 : t);
          columns[component].push(value === null || value === undefined ? NaN : value);
        }
      }
    }
    if (mpColumn.length === 0) {
      return Promise.reject(new ApiError(400, "bad_request", "empty series request"));
    }
    const arrays: Record<string, unknown> = {
      modelpoint: mpColumn,
      t: Int32Array.from(tColumn),
    };
    for (const component of q.components) arrays[component] = Float64Array.from(columns[component]);
    return Promise.resolve(tableFromArrays(arrays as never) as unknown as Table);
  }

  explain(q: ExplainQuery): Promise<unknown> {
    this.calls.explain.push(q);
    if (!this.caps.explain) {
      return Promise.reject(new ApiError(501, "unsupported", "explain() is not available"));
    }
    const trace = q.run === this.doc.b.run ? TRACE_B : TRACE_A;
    const root = (trace as { root?: { id?: string; modelpoint?: { key?: string } } }).root;
    if (
      q.component !== root?.id ||
      q.modelpoint !== root?.modelpoint?.key ||
      q.t !== 0
    ) {
      return Promise.reject(
        new ApiError(404, "no_trace", `no pre-baked trace for ${q.component}/${q.modelpoint}`),
      );
    }
    return Promise.resolve(trace);
  }
}
