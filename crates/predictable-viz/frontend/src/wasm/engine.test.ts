import { describe, expect, it } from "vitest";

import { ApiError, InlineDataSource, isUnsupported, type InlinePayload } from "../datasource";
import {
  attachPackEngine,
  callEngine,
  decodeWasm,
  engineFromExports,
  tableFromColumns,
  type EngineInputs,
} from "./engine";
import { fakeEngine } from "./fakeEngine";

const inputs: EngineInputs = {
  sources: [{ name: "model.pir", text: "format = \"pir/1\"\n" }],
  assumptions: { mortality_loading: 1.0 },
  tables: [],
  modelpoints: "policy_number\nTA00001\n",
};

/** Answers the four ops with values a test can recognise. */
const handler = (request: Record<string, unknown>): unknown => {
  switch (request.op) {
    case "version":
      return { ok: true, version: "0.0.1", ir_version: "pir/1", chunk: 256 };
    case "explain":
      return {
        ok: true,
        trace: { kind: "trace", root: { component: request.component, value: 42 } },
      };
    case "run":
      return {
        ok: true,
        rows: 4,
        mp: ["A", "A", "B", "B"],
        t: [0, 1, 0, 1],
        columns: { bel: [1, 2, 3, 4], claims: [5, 6, 7, 8] },
        model_digest: "sha256:m",
        plan_digest: "sha256:p",
        dropped: [],
      };
    default:
      return { ok: false, kind: "unknown_op", error: `\`${request.op}\` is not an operation` };
  }
};

describe("the ABI", () => {
  it("round-trips a request and frees the response buffer", () => {
    const fake = fakeEngine(handler);
    const response = callEngine(fake, { op: "version" }) as { version: string };
    expect(response.version).toBe("0.0.1");
    expect(fake.requests[0]).toEqual({ op: "version" });
    expect(fake.outstanding()).toBe(0);
  });

  it("survives the guest growing its memory mid-call", () => {
    // The classic wasm bug: `memory.buffer` is detached by `grow`, so a view
    // taken before the call points at nothing. Every view here is taken after.
    const fake = fakeEngine(handler, { growAfter: 0 });
    const response = callEngine(fake, { op: "version" }) as { version: string };
    expect(response.version).toBe("0.0.1");
  });

  it("carries multi-byte text without truncating it", () => {
    const fake = fakeEngine((request) => ({ ok: true, version: String(request.note) }));
    const response = callEngine(fake, { op: "version", note: "réserve ×2 — 中文" }) as {
      version: string;
    };
    expect(response.version).toBe("réserve ×2 — 中文");
  });
});

describe("engineFromExports", () => {
  it("reports the engine's own version, not the page's", () => {
    const engine = engineFromExports(fakeEngine(handler), inputs);
    expect(engine.version()).toBe("0.0.1");
  });

  it("sends the whole model with every explain, and asks for the whole tree", async () => {
    const fake = fakeEngine(handler);
    const engine = engineFromExports(fake, inputs);
    const trace = (await engine.explain({ component: "model.bel", modelpoint: "TA00001", t: 3 })) as {
      root: { component: string };
    };
    expect(trace.root.component).toBe("model.bel");
    const request = fake.requests.at(-1)!;
    expect(request.op).toBe("explain");
    expect(request.inputs).toEqual(inputs);
    expect(request.t).toBe(3);
    expect(request.depth).toBe(-1);
  });

  it("returns series as an Arrow table the screens can read", async () => {
    const engine = engineFromExports(fakeEngine(handler), inputs);
    const table = await engine.series({ components: ["bel"] });
    expect(table.numRows).toBe(4);
    expect(table.getChild("bel")!.get(2)).toBe(3);
    expect(String(table.getChild("mp")!.get(2))).toBe("B");
    // Only what was asked for: an unrequested column is not silently included.
    expect(table.getChild("claims")).toBeNull();
  });

  it("applies a t window without a second projection", async () => {
    const fake = fakeEngine(handler);
    const engine = engineFromExports(fake, inputs);
    const table = await engine.series({ components: ["bel"], tFrom: 1 });
    expect(table.numRows).toBe(2);
    expect([...table.getChild("t")!].map(Number)).toEqual([1, 1]);
    expect(fake.requests.filter((r) => r.op === "run")).toHaveLength(1);
  });

  it("turns an engine failure into the ApiError the screens already handle", async () => {
    const engine = engineFromExports(
      fakeEngine((request) =>
        request.op === "version"
          ? { ok: true, version: "0.0.1" }
          : { ok: false, kind: "unknown_component", error: "`nope` is not an output" },
      ),
      inputs,
    );
    await expect(
      engine.explain({ component: "nope", modelpoint: "TA00001", t: 0 }),
    ).rejects.toMatchObject({ status: 404, kind: "unknown_component" });
  });

  it("maps an unknown op to 501 so `isUnsupported` keeps working", () => {
    const engine = engineFromExports(fakeEngine(handler), inputs);
    return engine
      .series({ components: ["bel"] })
      .then(() => engine)
      .then(async () => {
        const broken = engineFromExports(
          fakeEngine((r) =>
            r.op === "version"
              ? { ok: true, version: "0.0.1" }
              : { ok: false, kind: "unsupported", error: "no" },
          ),
          inputs,
        );
        const error = await broken
          .explain({ component: "x", modelpoint: "A", t: 0 })
          .catch((e: unknown) => e);
        expect(isUnsupported(error)).toBe(true);
      });
  });
});

describe("tableFromColumns", () => {
  it("refuses a component the engine did not return", () => {
    expect(() =>
      tableFromColumns(
        {
          rows: 1,
          mp: ["A"],
          t: [0],
          columns: { bel: [1] },
          model_digest: "",
          plan_digest: "",
          dropped: [],
        },
        ["claims"],
      ),
    ).toThrow(ApiError);
  });
});

describe("attaching an engine to a pack", () => {
  const payload = (engine?: unknown): InlinePayload =>
    ({
      format: "pvf/1",
      kind: "pack",
      manifest: {},
      graph: { nodes: [], edges: [] },
      runs: [],
      engine,
    }) as unknown as InlinePayload;

  it("leaves a pack without an engine exactly as it was", async () => {
    const source = new InlineDataSource(payload());
    const same = await attachPackEngine(source, payload());
    expect(same).toBe(source);
    expect(await source.capabilities()).toEqual({
      explain: false,
      recompute: false,
      sensitivity: false,
    });
  });

  it("upgrades capabilities in place when the pack carries one", async () => {
    const source = new InlineDataSource(payload());
    // `attach` is what the upgrade *is*; instantiating a real module needs a
    // built artefact, which the Rust pack test covers end to end under node.
    source.attach(engineFromExports(fakeEngine(handler), inputs));
    expect(await source.capabilities()).toEqual({
      explain: true,
      recompute: true,
      sensitivity: true,
    });
    const trace = (await source.explain({ component: "model.bel", modelpoint: "A", t: 0 })) as {
      kind: string;
    };
    expect(trace.kind).toBe("trace");
  });

  it("decodes the embedded engine to the bytes that were exported", () => {
    // `AGFzbQEAAAA=` is the four-byte wasm magic plus a zero version word: the
    // first thing a browser checks and the first thing a corrupt base64 breaks.
    expect([...decodeWasm("AGFzbQEAAAA=")]).toEqual([0, 97, 115, 109, 1, 0, 0, 0]);
  });
});
