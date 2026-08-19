// The shared `DataSource` contract (T31 ↔ T32).
//
// These tests are the agreement itself: the run-diff types are checked against
// the diff's own JSON writer, and the inline/wasm seams are checked for the two
// properties the Rust conformance suite also enforces — capabilities never lie,
// and no screen can tell which implementation it was handed.

import { tableFromArrays, tableToIPC, type Table } from "apache-arrow";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  ApiError,
  HttpDataSource,
  InlineDataSource,
  PAYLOAD_ELEMENT_ID,
  base64ToBytes,
  isUnsupported,
  readInlinePayload,
  selectDataSource,
  type ExplainQuery,
  type InlinePayload,
  type SeriesQuery,
  type WasmEngine,
} from "./datasource";
import { TERM_ANNUAL, TERM_ANNUAL_RUNDIFF } from "./test/fixtures";

/** The Arrow a pack embeds: `mp`, `t`, then one column per component. */
function ipc(): string {
  const table = tableFromArrays({
    mp: ["TA00001", "TA00001", "TA00002", "TA00002"],
    t: Int32Array.from([0, 1, 0, 1]),
    reserve: Float64Array.from([151.39, 160.5, 240.25, 250.75]),
    bel: Float64Array.from([10.5, 11.5, 20.5, 21.5]),
  });
  const bytes = tableToIPC(table, "stream");
  let binary = "";
  for (const b of bytes) binary += String.fromCharCode(b);
  return btoa(binary);
}

function payload(over: Partial<InlinePayload> = {}): InlinePayload {
  return {
    format: "pvf/1",
    kind: "vizpack",
    manifest: { model_digest: "sha256:abc" },
    graph: TERM_ANNUAL,
    runs: [{ run: "runs/base", series_ipc: ipc() }],
    ...over,
  };
}

/** The rejection, as the `ApiError` the contract promises. */
async function rejection(p: Promise<unknown>): Promise<ApiError> {
  try {
    await p;
  } catch (e) {
    return e as ApiError;
  }
  throw new Error("expected the call to reject");
}

function rows(table: Table): Record<string, unknown>[] {
  return table.toArray().map((r) => JSON.parse(JSON.stringify(r)));
}

afterEach(() => {
  vi.unstubAllGlobals();
  document.getElementById(PAYLOAD_ELEMENT_ID)?.remove();
});

describe("the run-diff document (04-verify.md §5.4)", () => {
  it("types the diff writer's own output, field for field", () => {
    const doc = TERM_ANNUAL_RUNDIFF;
    expect(doc.kind).toBe("rundiff");
    expect(doc.summary.verdict).toBe("diverged");
    expect(doc.a.system).toBe("predictable");
    expect(doc.summary.cells.diverged).toBeGreaterThan(0);

    // Exactly one root divergence, five inherited — the claim Screen 2 renders.
    const root = doc.findings.filter((f) => f.class === "root");
    expect(root).toHaveLength(doc.summary.root_divergences);
    expect(root[0].component).toBe("model.premium_income");
    expect(root[0].class_basis).toBe("partial_graph");
    expect(root[0].t_first).toBe(0);
    expect(root[0].exemplar.mp_key).toMatch(/^TA/);
    expect(root[0].hypotheses[0].code).toBe("H0101");
    expect(root[0].hypotheses[0].evidence_support.held).toBeGreaterThan(0);
    expect(root[0].contribution?.method).toBe("delta_sum_ratio");
    expect(root[0].explained_by_model_change?.changed).toBe(true);
  });

  it("declares every field the writer emits, so nothing is silently dropped", () => {
    // A field present in the JSON but missing from the interface would be
    // invisible to the screen; this catches the writer moving ahead of us.
    const declared = new Set([
      "id",
      "class",
      "category",
      "class_basis",
      "component",
      "source_component",
      "affects_outputs",
      "t_first",
      "t_range",
      "n_modelpoints",
      "n_cells",
      "exemplar",
      "worst",
      "contribution",
      "explained_by_model_change",
      "message",
      "hypotheses",
      "explain_command",
    ]);
    for (const finding of TERM_ANNUAL_RUNDIFF.findings) {
      for (const key of Object.keys(finding)) expect(declared).toContain(key);
      expect(new Set(Object.keys(finding))).toEqual(declared);
    }
  });

  it("keeps the manifest reconciliation banner's inputs on both sides", () => {
    for (const side of [TERM_ANNUAL_RUNDIFF.a, TERM_ANNUAL_RUNDIFF.b]) {
      expect(side.manifest).toMatch(/^sha256:/);
      expect(side.model_digest).toMatch(/^sha256:/);
      expect(side.engine_version).not.toBe("");
    }
    // The two runs differ, which is what the banner must say first.
    expect(TERM_ANNUAL_RUNDIFF.a.manifest).not.toBe(TERM_ANNUAL_RUNDIFF.b.manifest);
  });
});

describe("HttpDataSource.diff", () => {
  it("asks the server for the two runs and returns the document", async () => {
    const fetchMock = vi.fn(
      async () =>
        new Response(JSON.stringify(TERM_ANNUAL_RUNDIFF), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const doc = await new HttpDataSource("tok").diff("good/runs/base", "bad/runs/base");
    expect(doc.findings[0].id).toBe("F001");

    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/api/diff?a=good%2Fruns%2Fbase&b=bad%2Fruns%2Fbase");
    expect((init.headers as Record<string, string>)["X-Predictable-Token"]).toBe("tok");
  });

  it("surfaces a 501 as an unsupported capability, not as a failure", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async () =>
          new Response(
            JSON.stringify({
              kind: "unsupported",
              error: "`diff` is not available",
            }),
            {
              status: 501,
            },
          ),
      ),
    );
    const err = await rejection(new HttpDataSource("tok").diff("a", "b"));
    expect(err).toBeInstanceOf(ApiError);
    expect(isUnsupported(err)).toBe(true);
    expect(err.kind).toBe("unsupported");
  });
});

describe("InlineDataSource — the governance pack (§1.4)", () => {
  it("answers the graph and a component from the embedded document", async () => {
    const source = new InlineDataSource(payload());
    expect((await source.graph()).nodes.length).toBe(TERM_ANNUAL.nodes.length);
    expect(await source.manifest()).toEqual({ model_digest: "sha256:abc" });

    const id = TERM_ANNUAL.edges[0].to;
    const doc = await source.component(id);
    expect(doc.node.id).toBe(id);
    expect(doc.upstream).toContain(TERM_ANNUAL.edges[0].from);
  });

  it("404s an unknown component instead of returning an empty neighbourhood", async () => {
    const err = await rejection(new InlineDataSource(payload()).component("model.nope"));
    expect(err).toBeInstanceOf(ApiError);
    expect(err.status).toBe(404);
  });

  it("projects the embedded Arrow rather than recomputing anything", async () => {
    const source = new InlineDataSource(payload());
    const table = await source.series({
      components: ["reserve"],
      modelpoints: ["TA00001"],
    });
    expect(rows(table)).toEqual([
      { mp: "TA00001", t: 0, reserve: 151.39 },
      { mp: "TA00001", t: 1, reserve: 160.5 },
    ]);
    // Columns come back in the order asked for, and `bel` is not smuggled in.
    expect(table.schema.fields.map((f) => f.name)).toEqual(["mp", "t", "reserve"]);
  });

  it("honours the t window and rejects a component the pack does not carry", async () => {
    const source = new InlineDataSource(payload());
    const table = await source.series({
      components: ["bel"],
      tFrom: 1,
      tTo: 1,
    });
    expect(rows(table)).toEqual([
      { mp: "TA00001", t: 1, bel: 11.5 },
      { mp: "TA00002", t: 1, bel: 21.5 },
    ]);
    const err = await rejection(source.series({ components: ["qx"] }));
    expect(err.status).toBe(404);
    const missing = await rejection(source.series({ run: "runs/other", components: ["bel"] }));
    expect(missing.status).toBe(404);
  });

  it("reports no capability it cannot honour, and rejects with 501 to match", async () => {
    const source = new InlineDataSource(payload());
    expect(await source.capabilities()).toEqual({
      explain: false,
      recompute: false,
      sensitivity: false,
    });
    expect(
      isUnsupported(
        await rejection(source.explain({ component: "reserve", modelpoint: "TA00001", t: 0 })),
      ),
    ).toBe(true);
    expect(isUnsupported(await rejection(source.diff("a", "b")))).toBe(true);
  });

  it("serves a pre-baked trace, and says which cells were not baked", async () => {
    const source = new InlineDataSource(
      payload({
        traces: [
          {
            component: "reserve",
            modelpoint: "TA00001",
            t: 0,
            trace: { kind: "Component", name: "reserve", value: 151.39 },
          },
        ],
      }),
    );
    expect((await source.capabilities()).explain).toBe(true);
    expect(
      await source.explain({
        component: "reserve",
        modelpoint: "TA00001",
        t: 0,
      }),
    ).toEqual({
      kind: "Component",
      name: "reserve",
      value: 151.39,
    });

    const err = await rejection(
      source.explain({ component: "reserve", modelpoint: "TA00001", t: 7 }),
    );
    expect(err.status).toBe(404);
    expect(err.kind).toBe("not_prebaked");
    expect(err.message).toContain("--engine wasm");
  });

  it("serves an embedded run diff to Screen 2 with no server present", async () => {
    const source = new InlineDataSource(
      payload({
        diffs: [{ a: "good/runs/base", b: "bad/runs/base", doc: TERM_ANNUAL_RUNDIFF }],
      }),
    );
    const doc = await source.diff("good/runs/base", "bad/runs/base");
    expect(doc.summary.verdict).toBe("diverged");
    const err = await rejection(source.diff("good/runs/base", "other"));
    expect(err.status).toBe(404);
  });
});

describe("attaching a wasm engine (§1.5)", () => {
  const engine = (): WasmEngine & { calls: string[] } => {
    const calls: string[] = [];
    return {
      calls,
      version: () => "0.0.1-wasm",
      explain: async (q: ExplainQuery) => {
        calls.push(`explain:${q.component}@${q.t}`);
        return { kind: "Component", name: q.component, value: 1 };
      },
      series: async (q: SeriesQuery) => {
        calls.push(`series:${q.components.join(",")}`);
        return tableFromArrays({
          t: Int32Array.from([0]),
          x: Float64Array.from([1]),
        });
      },
    };
  };

  it("upgrades the same source in place rather than swapping the class", async () => {
    const source = new InlineDataSource(payload());
    const wasm = engine();
    expect(source.attach(wasm)).toBe(source);
    expect(await source.capabilities()).toEqual({
      explain: true,
      recompute: true,
      sensitivity: true,
    });
  });

  it("routes explain() and series() to the engine once attached", async () => {
    const wasm = engine();
    const source = new InlineDataSource(payload()).attach(wasm);
    // A cell that was never pre-baked now answers, which is the whole point.
    expect(
      await source.explain({
        component: "reserve",
        modelpoint: "TA00001",
        t: 9,
      }),
    ).toEqual({
      kind: "Component",
      name: "reserve",
      value: 1,
    });
    await source.series({ components: ["reserve"] });
    expect(wasm.calls).toEqual(["explain:reserve@9", "series:reserve"]);
  });
});

describe("mode selection", () => {
  function embed(doc: InlinePayload): void {
    const el = document.createElement("script");
    el.type = "application/json";
    el.id = PAYLOAD_ELEMENT_ID;
    el.textContent = JSON.stringify(doc);
    document.body.appendChild(el);
  }

  it("finds no payload when the page is served by the local server", () => {
    expect(readInlinePayload()).toBeNull();
    expect(selectDataSource()).toBeInstanceOf(HttpDataSource);
  });

  it("prefers an embedded payload over the network", () => {
    embed(payload());
    expect(readInlinePayload()?.kind).toBe("vizpack");
    expect(selectDataSource()).toBeInstanceOf(InlineDataSource);
  });

  it("decodes base64 Arrow the way the pack stores it", () => {
    const bytes = base64ToBytes(ipc());
    // Arrow IPC streams start with the continuation marker `0xFFFFFFFF`.
    expect(Array.from(bytes.slice(0, 4))).toEqual([255, 255, 255, 255]);
  });
});
