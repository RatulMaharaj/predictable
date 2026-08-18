// The `WasmEngine` half of the DataSource contract — 05-viz.md §1.5.
//
// WASM is not a fourth delivery mode. It is a capability: an `InlineDataSource`
// built from a governance pack's payload is upgraded in place with
// `attach(engine)`, and `capabilities()` flips from `{explain: <some baked
// traces>}` to `{explain, recompute, sensitivity}`. Nothing else in the app
// changes, which is the point of the seam in `datasource.ts`.
//
// ---------------------------------------------------------------------------
// The ABI
// ---------------------------------------------------------------------------
//
// `predictable-wasm` exports three functions and its memory:
//
//   pv_alloc(len) -> ptr          the host writes `len` UTF-8 bytes there
//   pv_call(ptr, len) -> ptr      consumes the request, returns a response
//   pv_free(ptr, len)             frees either
//
// A response buffer is a little-endian `u32` length followed by that many UTF-8
// bytes, and the caller frees it with `pv_free(ptr, 4 + len)`. There is no
// wasm-bindgen glue: a governance pack is one file, and generated glue would
// make its contents depend on a tool version the auditor cannot re-derive.
//
// Two consequences worth stating because they shape the code below:
//
//   1. `memory.buffer` is detached and replaced whenever the guest grows its
//      heap, so every view is created *after* the call that might have grown it.
//      Caching a `Uint8Array` over the buffer is the classic bug here.
//   2. Requests are self-contained: the model, the tables, the assumptions and
//      the modelpoints travel with every call. There is no session handle to go
//      stale, and the page cannot end up asking a mutated engine a question.

import { tableFromArrays, type Table } from "apache-arrow";
import {
  ApiError,
  InlineDataSource,
  type ExplainQuery,
  type InlinePayload,
  type SeriesQuery,
  type WasmEngine,
} from "../datasource";

/** The model, as `predictable-wasm` wants it: text, never paths. */
export interface EngineInputs {
  sources: { name: string; text: string }[];
  assumptions: Record<string, number>;
  tables: { name: string; text: string }[];
  modelpoints: string;
  allow_table_drift?: boolean;
}

/** What `predictable export --engine wasm` embeds in the payload. */
export interface EnginePayload {
  kind: string;
  version: string;
  built_from?: string;
  bytes?: number;
  wasm_base64: string;
  inputs: EngineInputs;
}

/** The exports the loader needs. Anything else the module exports is ignored. */
export interface EngineExports {
  memory: WebAssembly.Memory;
  pv_alloc(len: number): number;
  pv_free(ptr: number, len: number): void;
  pv_call(ptr: number, len: number): number;
}

/** `{ok: false}` responses carry a `kind` the UI maps onto a status code. */
interface Failure {
  ok: false;
  kind: string;
  error: string;
  diagnostics?: unknown[];
}

/** `kind` → HTTP status, so `isUnsupported()` and the 404 path keep working. */
const STATUS: Record<string, number> = {
  unknown_component: 404,
  unknown_modelpoint: 404,
  unknown_op: 501,
  unsupported: 501,
  bad_request: 400,
  check_failed: 422,
  bad_modelpoints: 422,
  no_timeline: 422,
  engine: 500,
};

/** Turn an engine failure into the `ApiError` the screens already handle. */
export function toApiError(failure: Failure): ApiError {
  return new ApiError(STATUS[failure.kind] ?? 500, failure.kind, failure.error);
}

/** The raw JSON call, over the length-prefixed buffer protocol. */
export function callEngine(exports: EngineExports, request: unknown): unknown {
  const body = new TextEncoder().encode(JSON.stringify(request));
  const ptr = exports.pv_alloc(body.length);
  // Fresh view: `pv_alloc` may have grown the memory and detached the old one.
  new Uint8Array(exports.memory.buffer, ptr, body.length).set(body);
  const response = exports.pv_call(ptr, body.length);
  const length = new DataView(exports.memory.buffer).getUint32(response, true);
  const text = new TextDecoder().decode(
    new Uint8Array(exports.memory.buffer, response + 4, length),
  );
  exports.pv_free(response, length + 4);
  return JSON.parse(text);
}

function unwrap(response: unknown): Record<string, unknown> {
  const value = response as Record<string, unknown>;
  if (value?.ok !== true) throw toApiError(value as unknown as Failure);
  return value;
}

/** The long-form columns the engine returns for `run` and `sensitivity`. */
export interface SeriesColumns {
  rows: number;
  mp: string[];
  t: number[];
  columns: Record<string, number[]>;
  model_digest: string;
  plan_digest: string;
  dropped: string[];
}

/**
 * Columns → an Arrow `Table`, which is what `DataSource.series()` promises.
 *
 * §9's exposed surface says `run(...)` returns Arrow IPC. It does not, here:
 * emitting IPC from the guest would put `arrow-ipc` inside the 1.5 MB budget to
 * produce a buffer this function would immediately unpack again. The *contract*
 * is unchanged — callers get a `Table` — and the pack's stored results are
 * still base64 Arrow IPC, as §1.4 requires.
 */
export function tableFromColumns(series: SeriesColumns, components: string[]): Table {
  const columns: Record<string, unknown> = {
    mp: series.mp,
    t: Int32Array.from(series.t),
  };
  for (const name of components.length ? components : Object.keys(series.columns)) {
    const column = series.columns[name];
    if (!column) {
      throw new ApiError(404, "unknown_component", `\`${name}\` is not in this run`);
    }
    columns[name] = Float64Array.from(column);
  }
  return tableFromArrays(columns as never) as unknown as Table;
}

/** Base64 → bytes. The pack embeds the engine as text; this is the only decode. */
export function decodeWasm(base64: string): Uint8Array<ArrayBuffer> {
  const binary = atob(base64);
  const bytes = new Uint8Array(new ArrayBuffer(binary.length));
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

/**
 * A `WasmEngine` over already-instantiated exports.
 *
 * Split from `loadEngine` so the whole marshalling layer is testable without a
 * `.wasm` file: the tests drive it with a JS implementation of the same three
 * functions, and the real module is exercised by the Rust gate
 * (`crates/predictable-wasm/tests/wasm_gate.rs`) and by the pack test that runs
 * the page's loader under node.
 */
export function engineFromExports(exports: EngineExports, inputs: EngineInputs): WasmEngine {
  const version = String(
    (unwrap(callEngine(exports, { op: "version" })) as { version: string }).version,
  );
  return {
    version: () => version,

    async explain(query: ExplainQuery): Promise<unknown> {
      const response = unwrap(
        callEngine(exports, {
          op: "explain",
          inputs,
          component: query.component,
          modelpoint: query.modelpoint,
          t: query.t,
          // The whole tree: a pack is read by someone who wants the derivation,
          // and the depth control is a rendering choice the panel makes later.
          depth: -1,
        }),
      );
      return response.trace;
    },

    async series(query: SeriesQuery): Promise<Table> {
      const response = unwrap(
        callEngine(exports, {
          op: "run",
          inputs,
          components: query.components,
          modelpoints: query.modelpoints ?? [],
        }),
      ) as unknown as SeriesColumns;
      const table = tableFromColumns(response, query.components);
      // `t` filtering is done here rather than in the guest: the projection is
      // the whole horizon either way, and a `t` window is a view concern.
      return sliceByPeriod(table, query);
    },
  };
}

/** `tFrom`/`tTo` applied to a computed table, without another projection. */
export function sliceByPeriod(table: Table, query: SeriesQuery): Table {
  if (query.tFrom === undefined && query.tTo === undefined) return table;
  const t = table.getChild("t")!;
  const mp = table.getChild("mp")!;
  const rows: number[] = [];
  for (let i = 0; i < table.numRows; i += 1) {
    const period = Number(t.get(i));
    if (query.tFrom !== undefined && period < query.tFrom) continue;
    if (query.tTo !== undefined && period > query.tTo) continue;
    rows.push(i);
  }
  const columns: Record<string, unknown> = {
    mp: rows.map((i) => String(mp.get(i))),
    t: Int32Array.from(rows.map((i) => Number(t.get(i)))),
  };
  for (const field of table.schema.fields) {
    if (field.name === "mp" || field.name === "t") continue;
    const column = table.getChild(field.name)!;
    columns[field.name] = Float64Array.from(rows.map((i) => Number(column.get(i))));
  }
  return tableFromArrays(columns as never) as unknown as Table;
}

/**
 * Instantiate the embedded engine and build a `WasmEngine` from it.
 *
 * No imports are supplied and none are needed: the module has no filesystem, no
 * clock and no host calls at all, which is what makes a `file://` page able to
 * run it with no server and no COOP/COEP headers.
 */
export async function loadEngine(
  wasm: BufferSource,
  inputs: EngineInputs,
): Promise<WasmEngine> {
  const { instance } = await WebAssembly.instantiate(wasm, {});
  return engineFromExports(instance.exports as unknown as EngineExports, inputs);
}

/**
 * The one call `main.tsx` needs: upgrade an inline pack in place when it
 * carries an engine, and leave it exactly as it was when it does not.
 *
 * Returns the same source either way, so mode selection stays a single
 * expression and no screen ever branches on which implementation it got.
 */
export async function attachPackEngine(
  source: InlineDataSource,
  payload: InlinePayload & { engine?: EnginePayload | null },
): Promise<InlineDataSource> {
  if (!payload.engine?.wasm_base64) return source;
  const engine = await loadEngine(decodeWasm(payload.engine.wasm_base64), payload.engine.inputs);
  return source.attach(engine);
}
