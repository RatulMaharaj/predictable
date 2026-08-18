import { describe, expect, it } from "vitest";
import ELK from "elkjs/lib/elk.bundled.js";
import { TERM_ANNUAL } from "../test/fixtures";
import {
  CACHE_PREFIX,
  LayoutCache,
  LAYOUT_OPTIONS,
  fromElkResult,
  layoutCovers,
  routeFor,
  toElkGraph,
  type Layout,
} from "./layout";
import { MemoryStore } from "../test/fixtures";
import { resolveLayout } from "./useLayout";

describe("the ELK input", () => {
  it("asks for a left-to-right layered network-simplex layout", () => {
    expect(LAYOUT_OPTIONS["elk.algorithm"]).toBe("layered");
    expect(LAYOUT_OPTIONS["elk.direction"]).toBe("RIGHT");
    expect(LAYOUT_OPTIONS["elk.layered.nodePlacement.strategy"]).toBe("NETWORK_SIMPLEX");
    // An unread input must not be packed into a column of its own to the right.
    expect(LAYOUT_OPTIONS["elk.separateConnectedComponents"]).toBe("false");
  });

  it("pins every node to the engine's own depth", () => {
    const graph = toElkGraph(TERM_ANNUAL);
    expect(graph.layoutOptions["elk.partitioning.activate"]).toBe("true");
    for (const child of graph.children) {
      const node = TERM_ANNUAL.nodes.find((n) => n.id === child.id)!;
      expect(child.layoutOptions["elk.partitioning.partition"]).toBe(String(node.depth));
    }
  });

  it("drops self edges — the `t-1` loop is drawn on the node, not as a layer", () => {
    const graph = toElkGraph(TERM_ANNUAL);
    expect(graph.edges.some((e) => e.sources[0] === e.targets[0])).toBe(false);
    expect(TERM_ANNUAL.edges.some((e) => e.from === e.to)).toBe(true);
  });

  it("collapses parallel reads of the same component into one route", () => {
    const graph = toElkGraph(TERM_ANNUAL);
    expect(new Set(graph.edges.map((e) => e.id)).size).toBe(graph.edges.length);
    expect(graph.edges.length).toBeLessThan(TERM_ANNUAL.edges.length);
  });
});

describe("ELK actually lays the reference model out", () => {
  it("produces a left-to-right layout whose x order follows the evaluation depth", async () => {
    const elk = new ELK();
    const result = await elk.layout(toElkGraph(TERM_ANNUAL) as never);
    const layout = fromElkResult(result, TERM_ANNUAL.model_digest);

    expect(layoutCovers(layout, TERM_ANNUAL)).toBe(true);
    expect(layout.width).toBeGreaterThan(0);

    // The load-bearing property of §2.2: the columns an evaluation depth
    // occupies are strictly to the right of every shallower depth's, with no
    // overlap — so reading the picture left to right *is* reading the plan.
    const extent = new Map<number, { min: number; max: number }>();
    for (const node of TERM_ANNUAL.nodes) {
      const x = layout.nodes[node.id].x;
      const seen = extent.get(node.depth);
      if (!seen) extent.set(node.depth, { min: x, max: x });
      else {
        seen.min = Math.min(seen.min, x);
        seen.max = Math.max(seen.max, x);
      }
    }
    const columns = Array.from(extent).sort((a, b) => a[0] - b[0]);
    expect(columns.map(([d]) => d)).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8]);
    for (let i = 1; i < columns.length; i += 1) {
      expect(columns[i][1].min).toBeGreaterThan(columns[i - 1][1].max);
    }
  }, 30_000);
});

describe("edge routes", () => {
  const layout: Layout = {
    model_digest: "d",
    width: 200,
    height: 100,
    nodes: {
      a: { x: 0, y: 0, width: 40, height: 20 },
      b: { x: 100, y: 40, width: 40, height: 20 },
    },
    edges: {
      "a->b": { id: "a->b", points: [{ x: 40, y: 10 }, { x: 70, y: 25 }, { x: 100, y: 50 }] },
    },
  };
  const edge = (from: string, to: string) => ({
    from,
    to,
    lag: 0,
    at: false,
    stage: 1,
    via: "expr",
    span: { file: "m.pir", line: 1, col: 1, byte_start: 0, byte_end: 1 },
  });

  it("uses ELK's bend points when it has them", () => {
    expect(routeFor(layout, edge("a", "b")).length).toBe(3);
  });

  it("falls back to a straight right-edge-to-left-edge run", () => {
    const straight = routeFor({ ...layout, edges: {} }, edge("a", "b"));
    expect(straight).toEqual([
      { x: 40, y: 10 },
      { x: 100, y: 50 },
    ]);
  });

  it("returns nothing rather than throwing for an edge off the layout", () => {
    expect(routeFor(layout, edge("a", "ghost"))).toEqual([]);
  });
});

describe("the digest-keyed layout cache", () => {
  const layout: Layout = {
    model_digest: TERM_ANNUAL.model_digest,
    width: 10,
    height: 10,
    nodes: Object.fromEntries(
      TERM_ANNUAL.nodes.map((n) => [n.id, { x: 1, y: 2, width: 3, height: 4 }]),
    ),
    edges: {},
  };

  it("keys on the model digest and nothing else", () => {
    const store = new MemoryStore();
    new LayoutCache(store).put(layout);
    expect(Array.from(store.map.keys())).toEqual([CACHE_PREFIX + TERM_ANNUAL.model_digest]);
    expect(new LayoutCache(store).get(TERM_ANNUAL.model_digest)).toEqual(layout);
    expect(new LayoutCache(store).get("sha256:other")).toBeUndefined();
  });

  it("survives corrupt stored JSON", () => {
    const store = new MemoryStore();
    store.setItem(CACHE_PREFIX + "x", "{not json");
    expect(new LayoutCache(store).get("x")).toBeUndefined();
  });

  it("rejects a cached layout that does not cover the document", () => {
    const partial = { ...layout, nodes: { "model.qx": layout.nodes["model.qx"] } };
    expect(layoutCovers(partial, TERM_ANNUAL)).toBe(false);
  });

  it("a cache hit means the worker is never asked", async () => {
    const store = new MemoryStore();
    const cache = new LayoutCache(store);
    cache.put(layout);
    let calls = 0;
    const engine = {
      layout: async () => {
        calls += 1;
        return layout;
      },
    };
    const first = await resolveLayout(TERM_ANNUAL, cache, engine);
    expect(first.fromCache).toBe(true);
    expect(calls).toBe(0);
  });

  it("a cache miss lays out once and writes the result back", async () => {
    const store = new MemoryStore();
    const cache = new LayoutCache(store);
    let calls = 0;
    const engine = {
      layout: async () => {
        calls += 1;
        return layout;
      },
    };
    expect((await resolveLayout(TERM_ANNUAL, cache, engine)).fromCache).toBe(false);
    expect(calls).toBe(1);
    // Second open — the "instant, and positions stable across sessions" promise.
    expect((await resolveLayout(TERM_ANNUAL, cache, engine)).fromCache).toBe(true);
    expect(calls).toBe(1);
  });

  it("never breaks the screen when the store refuses to write", () => {
    const full = {
      getItem: () => null,
      setItem: () => {
        throw new Error("QuotaExceededError");
      },
      removeItem: () => {},
    };
    expect(() => new LayoutCache(full).put(layout)).not.toThrow();
  });
});

describe("fromElkResult", () => {
  it("flattens sections into a polyline, start → bends → end", () => {
    const layout = fromElkResult(
      {
        width: 5,
        height: 6,
        children: [{ id: "a", x: 1, y: 2, width: 3, height: 4 }],
        edges: [
          {
            id: "a->b",
            sections: [
              {
                startPoint: { x: 0, y: 0 },
                bendPoints: [{ x: 1, y: 1 }],
                endPoint: { x: 2, y: 2 },
              },
            ],
          },
        ],
      },
      "sha256:zz",
    );
    expect(layout.model_digest).toBe("sha256:zz");
    expect(layout.nodes.a).toEqual({ x: 1, y: 2, width: 3, height: 4 });
    expect(layout.edges["a->b"].points).toEqual([
      { x: 0, y: 0 },
      { x: 1, y: 1 },
      { x: 2, y: 2 },
    ]);
  });
});
