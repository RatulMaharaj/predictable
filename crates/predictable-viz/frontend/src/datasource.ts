// The `DataSource` contract — 05-viz.md §1.1, §1.4, §1.5.
//
// The app is written against this interface; the only difference between the
// three delivery modes is which implementation is injected. The Rust half of the
// same contract lives in `src/source.rs`, and both are held to the conformance
// suite in `src/conformance.rs`.
//
// ---------------------------------------------------------------------------
// THE SHARED CONTRACT (T31 run-diff screen ↔ T32 wasm + static export)
// ---------------------------------------------------------------------------
//
// This file is the single seam both tasks build against. It is frozen in the
// following sense: T31 and T32 may add *screens* and *implementations*, but the
// shapes below change only by agreement between them.
//
// 1. `DataSource` is the whole surface. Screens receive one, never construct
//    one, and never branch on which implementation they got. `main.tsx` and
//    `drilldown/mount.tsx` are the only injection points, and both obtain the
//    source from `selectDataSource()`.
//
// 2. Three implementations, one behaviour:
//      · `HttpDataSource`   — the local axum server (§1.2). Exists today.
//      · `InlineDataSource` — the single-file governance pack (§1.4): the
//        payload is embedded in the page, results are base64 Arrow IPC, and
//        `explain()` answers from pre-baked traces. **T32 owns this**, and owns
//        the export writer that produces `InlinePayload`.
//      · a wasm-backed source — *not a fourth class*. §1.5 says wasm is not a
//        delivery mode but a capability: `InlineDataSource.attach(engine)`
//        upgrades an inline pack in place, flipping `capabilities()` to
//        `{explain, recompute, sensitivity}` and routing those calls to the
//        engine. T32 implements `WasmEngine`; nothing else in the app changes.
//
// 3. Capabilities never lie (the rule the Rust conformance suite enforces). A
//    source reporting `explain: false` must reject `explain()` with an
//    `ApiError` whose status is 501, so the UI greys the button out rather than
//    offering a dead one. Use `isUnsupported(err)` to test for that, never a
//    string match on the message.
//
// 4. Run-diff data access (T31): `diff(a, b)` returns the `pvf/1` `rundiff`
//    document of 04-verify.md §5.4, typed here as `RunDiffDoc`. The types below
//    mirror `crates/predictable-rundiff/src/{lib,model,hypothesis}.rs` field for
//    field. Screen 2 renders that document; it does **not** recompute deltas,
//    re-rank findings, or re-derive the verdict. The one derived quantity §4.2
//    permits (cohort detection over the contributors) is a chart-only quantity
//    and is labelled as a hypothesis in the UI — it lives in `src/rundiff/`,
//    which T31 owns, not here.
//
// 5. Adding a call means adding it to `DataSource`, to `HttpDataSource`, to
//    `InlineDataSource`, and to the Rust `source.rs` + conformance suite. A call
//    that only one implementation can answer belongs behind a capability, not
//    behind an `instanceof`.

import { tableFromIPC, tableFromArrays, type Table } from "apache-arrow";

export interface SpanDoc {
  file: string;
  line: number;
  col: number;
  byte_start: number;
  byte_end: number;
}

/** `meta.source` (IR §11.1) — which Prophet variable this component came from. */
export interface SourceRef {
  system?: string;
  library?: string;
  variable?: string;
  file?: string;
}

/** A lint the checker raised against this component, drawn on the node (§4.1). */
export interface NodeLint {
  code: string;
  severity: string;
  message: string;
}

export interface GraphNode {
  id: string;
  name: string;
  module: string;
  /** `InputModelpoint` / `InputAssumption` / `InputTimeline` / `Derived` / `Output` / `Table`. */
  kind: string;
  dtype: string;
  shape: string;
  unit: string;
  timing?: "start" | "end";
  stage: number;
  expr?: string;
  init?: string;
  doc?: string;
  tags: string[];
  span?: SpanDoc;
  /** Prophet provenance, when the migration tool wrote it (§2.1). */
  source?: SourceRef;
  /** Checker lints for this component (§4.1: lints live on the graph). */
  lints?: NodeLint[];
  declaration_index: number;
  depth: number;
  retention?: string;
  hoistable?: boolean;
}

export interface GraphEdge {
  from: string;
  to: string;
  /** `k` for `x[t-k]`, `0` for a same-period read, `null` for a table read. */
  lag: number | null;
  /** True when the read was an absolute `At(x, k)` seed edge. */
  at: boolean;
  stage: number;
  via: string;
  span: SpanDoc;
}

export interface GraphDoc {
  ir_version: string;
  model_digest: string;
  plan_digest: string;
  order_digest: string;
  modules: string[];
  nodes: GraphNode[];
  edges: GraphEdge[];
  /** The planner's own order, bucketed by depth. Never recomputed here. */
  layers: string[][];
}

export interface ComponentDoc {
  node: GraphNode;
  upstream: string[];
  downstream: string[];
}

export interface Capabilities {
  explain: boolean;
  recompute: boolean;
  sensitivity: boolean;
}

export interface SeriesQuery {
  run?: string;
  components: string[];
  modelpoints?: string[];
  tFrom?: number;
  tTo?: number;
}

export interface ExplainQuery {
  run?: string;
  component: string;
  modelpoint: string;
  t: number;
}

// ---------------------------------------------------------------------------
// The run-diff document (04-verify.md §5.4) — T31's data access.
// ---------------------------------------------------------------------------

/** One side of the comparison, as the diff recorded it. */
export interface DiffSide {
  path: string;
  run: string;
  /** `predictable` | `prophet` — §4.2 makes a `.rpt` a first-class side A. */
  system: string;
  manifest: string;
  engine_version: string;
  model_digest: string;
}

/** `pass` | `fail` | `incomparable`, and the exit code. */
export type Verdict = string;

/** `root` | `inherited` | `structural` | `tolerance_only`. */
export type FindingClass = string;

export interface SetCounts {
  a: number;
  b: number;
  common: number;
  only_a: number;
  only_b: number;
}

export interface CellCounts {
  compared: number;
  diverged: number;
  absorbed_by_override: number;
  max_abs: number;
  max_rel: number;
}

/** `summary.outputs[]` — the per-output movement the headline bars render. */
export interface OutputTotal {
  component: string;
  a_total: number;
  b_total: number;
  abs: number;
  rel: number;
  within_tolerance: boolean;
}

/** `summary.emit_mismatch` — banner-worthy, and not by itself a failure. */
export interface EmitMismatch {
  a: string;
  b: string;
  a_component_set_digest: string;
  b_component_set_digest: string;
}

export interface FirstDivergence {
  component: string;
  t: number;
  mp_key: string;
}

/** One diverging cell. `abs`/`rel` are `null` when not finite. */
export interface DiffCell {
  mp_key: string;
  mp_row: number;
  t: number;
  a: unknown;
  b: unknown;
  abs: number | null;
  rel: number | null;
  category: string;
}

/** `findings[].contribution` — the contributor bars of §4.2. */
export interface Contribution {
  output: string;
  share_of_total_delta: number;
  /** How the share was computed; shown, because an unstated method is a lie. */
  method: string;
}

/** `findings[].explained_by_model_change` — the attribution panel of §4.2. */
export interface ExplainedByModelChange {
  changed: boolean;
  what: string[];
}

export interface HypothesisSupport {
  tested: number;
  held: number;
}

/** `H0101` … `H0501` (04-verify.md §5.5), proposals with their evidence. */
export interface Hypothesis {
  code: string;
  message: string;
  /** `high` | `medium` | `low`. */
  confidence: string;
  evidence: unknown;
  evidence_support: HypothesisSupport;
  suggested_edit: unknown;
}

export interface Finding {
  id: string;
  class: FindingClass;
  category: string;
  /** `ir_graph` | `partial_graph` | `no_graph_earliest_t`. */
  class_basis: string;
  component: string;
  source_component: string | null;
  affects_outputs: string[];
  t_first: number;
  t_range: [number, number];
  n_modelpoints: number;
  n_cells: number;
  exemplar: DiffCell;
  worst: DiffCell;
  contribution: Contribution | null;
  explained_by_model_change: ExplainedByModelChange | null;
  message: string;
  hypotheses: Hypothesis[];
  explain_command: string;
}

export interface OneSided {
  component: string;
  reason: string;
}

export interface StructuralDiff {
  only_in_a: OneSided[];
  only_in_b: OneSided[];
  modelpoints_only_in_a: string[];
  modelpoints_only_in_b: string[];
}

export interface DiffSummary {
  verdict: Verdict;
  incomparable: string[];
  modelpoints: SetCounts;
  components: SetCounts;
  emit_mismatch: EmitMismatch | null;
  cells: CellCounts;
  outputs: OutputTotal[];
  root_divergences: number;
  first_divergence: FirstDivergence | null;
  /** False means `explained_by_model_change` is silent, not empty. */
  model_attribution_available: boolean;
  graph_available: boolean;
}

/**
 * The `pvf/1` `rundiff` document. Rendered verbatim: the screen never
 * recomputes a delta or re-ranks a finding.
 */
export interface RunDiffDoc {
  format: string;
  kind: string;
  a: DiffSide;
  b: DiffSide;
  tolerance: unknown;
  mapping: unknown;
  summary: DiffSummary;
  findings: Finding[];
  findings_truncated?: { kept: number; of: number };
  structural: StructuralDiff;
}

export interface DataSource {
  manifest(): Promise<unknown>;
  graph(): Promise<GraphDoc>;
  component(name: string): Promise<ComponentDoc>;
  series(q: SeriesQuery): Promise<Table>;
  explain(q: ExplainQuery): Promise<unknown>;
  /** 04-verify.md §5.4. Rejects with a 501 `ApiError` where unsupported. */
  diff(a: string, b: string): Promise<RunDiffDoc>;
  capabilities(): Promise<Capabilities>;
}

export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly kind: string,
    message: string,
  ) {
    super(message);
  }
}

/** Talks to the local axum server (§1.2). All capabilities depend on the run. */
export class HttpDataSource implements DataSource {
  constructor(
    private readonly token: string = location.hash.slice(1),
    private readonly base: string = "",
  ) {}

  private async get(path: string, accept: string): Promise<Response> {
    const response = await fetch(`${this.base}${path}`, {
      headers: { Accept: accept, "X-Predictable-Token": this.token },
    });
    if (!response.ok) {
      const body = await response.json().catch(() => ({}));
      throw new ApiError(response.status, body.kind ?? "http", body.error ?? response.statusText);
    }
    return response;
  }

  private async json<T>(path: string): Promise<T> {
    return (await this.get(path, "application/json")).json() as Promise<T>;
  }

  manifest(): Promise<unknown> {
    return this.json("/api/manifest");
  }

  graph(): Promise<GraphDoc> {
    return this.json<GraphDoc>("/api/graph");
  }

  component(name: string): Promise<ComponentDoc> {
    return this.json<ComponentDoc>(`/api/component/${encodeURIComponent(name)}`);
  }

  capabilities(): Promise<Capabilities> {
    return this.json<Capabilities>("/api/capabilities");
  }

  /** Numbers arrive as an Arrow IPC stream and go zero-copy into typed arrays. */
  async series(q: SeriesQuery): Promise<Table> {
    const params = new URLSearchParams();
    params.set("components", q.components.join(","));
    if (q.run) params.set("run", q.run);
    if (q.modelpoints?.length) params.set("mp", q.modelpoints.join(","));
    if (q.tFrom !== undefined || q.tTo !== undefined) {
      params.set("t", `${q.tFrom ?? ""}..${q.tTo ?? ""}`);
    }
    const response = await this.get(`/api/series?${params}`, "application/vnd.apache.arrow.stream");
    return tableFromIPC(new Uint8Array(await response.arrayBuffer()));
  }

  async explain(q: ExplainQuery): Promise<unknown> {
    const response = await fetch(`${this.base}/api/explain`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Predictable-Token": this.token,
      },
      body: JSON.stringify(q),
    });
    if (!response.ok) {
      const body = await response.json().catch(() => ({}));
      throw new ApiError(response.status, body.kind ?? "http", body.error ?? response.statusText);
    }
    return response.json();
  }

  diff(a: string, b: string): Promise<RunDiffDoc> {
    return this.json<RunDiffDoc>(`/api/diff?a=${encodeURIComponent(a)}&b=${encodeURIComponent(b)}`);
  }
}

// ---------------------------------------------------------------------------
// The static / wasm half of the contract (§1.4, §1.5) — T32 owns what follows.
// ---------------------------------------------------------------------------

/** The 501 case: the source is honest, the capability is simply absent. */
export function isUnsupported(err: unknown): boolean {
  return err instanceof ApiError && err.status === 501;
}

/** A pre-baked `explain()` trace, exported with `--with-traces`. */
export interface InlineTrace {
  run?: string;
  component: string;
  modelpoint: string;
  t: number;
  /** The trace tree of 04-verify.md §3.2, verbatim. */
  trace: unknown;
}

/** One run's sampled results, as one base64 Arrow IPC stream (§1.4). */
export interface InlineRun {
  run: string;
  /**
   * Base64 Arrow IPC. Columns: `mp` (utf8), `t` (int), then one column per
   * exported component. Results are Arrow, never JSON — that is the size
   * discipline §1.4 sets.
   */
  series_ipc: string;
}

/** The whole embedded payload of a governance pack. */
export interface InlinePayload {
  format: string;
  kind: string;
  /** The four digests, shown as a visible header, not a tooltip (§1.4). */
  manifest: unknown;
  graph: GraphDoc;
  runs: InlineRun[];
  /** Pre-baked run diffs, `--include diff:<run>`. Keyed by both run ids. */
  diffs?: { a: string; b: string; doc: RunDiffDoc }[];
  traces?: InlineTrace[];
}

/**
 * The engine, when the pack was exported with `--engine wasm` (§1.5).
 *
 * T32 implements this over `predictable-wasm`. Attaching one is the *only*
 * difference between a frozen pack and a live one: it upgrades
 * `capabilities()` and takes over `explain()`/`series()`.
 */
export interface WasmEngine {
  /** Engine version, shown next to the pack's own manifest. */
  version(): string;
  /** A replay of one modelpoint (IR §11.2) — microseconds. */
  explain(q: ExplainQuery): Promise<unknown>;
  /** Re-run a projection in the page: sensitivity fans without a round trip. */
  series(q: SeriesQuery): Promise<Table>;
}

/**
 * A `DataSource` over an embedded payload — the single-file governance pack.
 *
 * Everything it answers is already in the file: no network at all. `explain()`
 * answers from the pre-baked traces, and rejects with a 404 `ApiError` for a
 * cell that was not baked — which is why `capabilities().explain` is true only
 * when there is *something* to explain. `attach()` an engine and both
 * restrictions go away.
 */
export class InlineDataSource implements DataSource {
  private engine: WasmEngine | null = null;
  private readonly tables = new Map<string, Table>();

  constructor(private readonly payload: InlinePayload) {}

  /** Upgrade the pack in place (§1.5). Returns `this` so it chains. */
  attach(engine: WasmEngine): this {
    this.engine = engine;
    return this;
  }

  capabilities(): Promise<Capabilities> {
    const live = this.engine !== null;
    return Promise.resolve({
      explain: live || (this.payload.traces?.length ?? 0) > 0,
      recompute: live,
      sensitivity: live,
    });
  }

  manifest(): Promise<unknown> {
    return Promise.resolve(this.payload.manifest);
  }

  graph(): Promise<GraphDoc> {
    return Promise.resolve(this.payload.graph);
  }

  component(name: string): Promise<ComponentDoc> {
    const doc = this.payload.graph;
    const node = doc.nodes.find((n) => n.id === name || n.name === name);
    if (!node) {
      return Promise.reject(
        new ApiError(404, "unknown_component", `unknown component \`${name}\``),
      );
    }
    const from = new Set(doc.edges.filter((e) => e.to === node.id).map((e) => e.from));
    const to = new Set(doc.edges.filter((e) => e.from === node.id).map((e) => e.to));
    return Promise.resolve({ node, upstream: [...from], downstream: [...to] });
  }

  /** Projects the embedded Arrow table. Never recomputes — that is `explain`. */
  async series(q: SeriesQuery): Promise<Table> {
    if (this.engine) return this.engine.series(q);
    const table = this.table(q.run);
    const mp = table.getChild("mp");
    const t = table.getChild("t");
    if (!mp || !t) {
      throw new ApiError(500, "bad_payload", "the embedded table has no `mp`/`t` columns");
    }
    for (const name of q.components) {
      if (!table.getChild(name)) {
        throw new ApiError(404, "unknown_component", `\`${name}\` is not in this pack`);
      }
    }
    const wanted = q.modelpoints?.length ? new Set(q.modelpoints) : null;
    const rows: number[] = [];
    for (let i = 0; i < table.numRows; i += 1) {
      const period = Number(t.get(i));
      if (q.tFrom !== undefined && period < q.tFrom) continue;
      if (q.tTo !== undefined && period > q.tTo) continue;
      if (wanted && !wanted.has(String(mp.get(i)))) continue;
      rows.push(i);
    }
    const columns: Record<string, unknown[] | Float64Array | Int32Array> = {
      mp: rows.map((i) => String(mp.get(i))),
      t: Int32Array.from(rows.map((i) => Number(t.get(i)))),
    };
    for (const name of q.components) {
      const column = table.getChild(name)!;
      columns[name] = Float64Array.from(rows.map((i) => Number(column.get(i))));
    }
    return tableFromArrays(columns as never) as unknown as Table;
  }

  async explain(q: ExplainQuery): Promise<unknown> {
    if (this.engine) return this.engine.explain(q);
    const baked = (this.payload.traces ?? []).find(
      (tr) =>
        tr.component === q.component &&
        tr.modelpoint === q.modelpoint &&
        tr.t === q.t &&
        (q.run === undefined || tr.run === undefined || tr.run === q.run),
    );
    if (baked) return baked.trace;
    if ((this.payload.traces?.length ?? 0) === 0) {
      throw new ApiError(501, "unsupported", "this pack was exported without an engine");
    }
    throw new ApiError(
      404,
      "not_prebaked",
      `no pre-baked trace for ${q.component} at ${q.modelpoint}/t=${q.t}; re-export with --engine wasm`,
    );
  }

  diff(a: string, b: string): Promise<RunDiffDoc> {
    const found = (this.payload.diffs ?? []).find((d) => d.a === a && d.b === b);
    if (found) return Promise.resolve(found.doc);
    if ((this.payload.diffs ?? []).length === 0) {
      return Promise.reject(
        new ApiError(501, "unsupported", "this pack was exported without a diff"),
      );
    }
    return Promise.reject(new ApiError(404, "unknown_run", `no embedded diff for ${a}..${b}`));
  }

  private table(run?: string): Table {
    const entry = run ? this.payload.runs.find((r) => r.run === run) : this.payload.runs[0];
    if (!entry) throw new ApiError(404, "unknown_run", `unknown run \`${run ?? ""}\``);
    let table = this.tables.get(entry.run);
    if (!table) {
      table = tableFromIPC(base64ToBytes(entry.series_ipc));
      this.tables.set(entry.run, table);
    }
    return table;
  }
}

/** Base64 → bytes, without pulling in a dependency for eight lines. */
export function base64ToBytes(b64: string): Uint8Array {
  const binary = atob(b64);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) out[i] = binary.charCodeAt(i);
  return out;
}

/** Where `predictable export --format html` writes the payload. */
export const PAYLOAD_ELEMENT_ID = "predictable-payload";

/**
 * The embedded payload, when the page is a governance pack. `null` means we are
 * being served by the local server instead.
 */
export function readInlinePayload(doc: Document = document): InlinePayload | null {
  const el = doc.getElementById(PAYLOAD_ELEMENT_ID);
  if (!el?.textContent?.trim()) return null;
  return JSON.parse(el.textContent) as InlinePayload;
}

/**
 * Mode selection (§1.1), and the app's only place that knows modes exist: an
 * inline payload wins when one was embedded; otherwise the token is in the URL
 * fragment and we talk to the local axum server.
 */
export function selectDataSource(doc: Document = document): DataSource {
  const payload = readInlinePayload(doc);
  if (payload) return new InlineDataSource(payload);
  return new HttpDataSource(location.hash.slice(1));
}
