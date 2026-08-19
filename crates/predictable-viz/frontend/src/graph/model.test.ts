import { describe, expect, it } from "vitest";
import { TERM_ANNUAL } from "../test/fixtures";
import {
  applyFilters,
  facets,
  impact,
  impactSet,
  indexGraph,
  neighbourhood,
  resolveId,
  visibleEdges,
  NO_FILTERS,
} from "./model";

const index = indexGraph(TERM_ANNUAL);

describe("the graph is the engine's, not ours", () => {
  it("indexes every node the plan sent", () => {
    expect(index.byId.size).toBe(TERM_ANNUAL.nodes.length);
    expect(TERM_ANNUAL.nodes.length).toBe(48);
  });

  it("keeps `layers` as the planner's own ordering, bucketed by depth", () => {
    // Not "recomputes the same answer" — literally the same arrays.
    for (const node of TERM_ANNUAL.nodes) {
      expect(TERM_ANNUAL.layers[node.depth]).toContain(node.name);
    }
  });

  it("never places a reader in a layer at or before a same-period `expr` dependency", () => {
    // The one documented exception is the `init` → stage-2 back-channel
    // (IR §8.2), which the renderer draws distinctly *because* it is the only
    // legal backward read. Assert both halves.
    const backwards: string[] = [];
    for (const edge of TERM_ANNUAL.edges) {
      if (edge.lag !== 0 || edge.from === edge.to) continue;
      const from = index.byId.get(edge.from);
      const to = index.byId.get(edge.to);
      if (!from || !to) continue;
      if (to.depth <= from.depth) backwards.push(`${edge.from}->${edge.to} via ${edge.via}`);
    }
    expect(backwards).toEqual(["model.bel->model.reserve via init"]);
  });
});

describe("impact overlay", () => {
  it("reports the transitive downstream closure of qx", () => {
    const ids = impactSet(index, "model.qx");
    expect(ids.has("model.num_pols_if")).toBe(true);
    expect(ids.has("model.deaths")).toBe(true);
    expect(ids.has("model.bel")).toBe(true);
    // Upstream of qx must never appear.
    expect(ids.has("model.age")).toBe(false);
    expect(ids.has("schema.mortality")).toBe(false);
  });

  it("does not count the seed itself, even when it is self-referential", () => {
    // `num_pols_if` reads `num_pols_if[t-1]`, so a naive closure includes it.
    const selfEdge = TERM_ANNUAL.edges.find(
      (e) => e.from === "model.num_pols_if" && e.to === "model.num_pols_if",
    );
    expect(selfEdge?.lag).toBe(1);
    expect(impactSet(index, "model.num_pols_if").has("model.num_pols_if")).toBe(false);
  });

  it("summarises as the sidebar phrases it", () => {
    const summary = impact(index, "model.qx");
    expect(summary.components).toBeGreaterThan(4);
    expect(summary.outputs).toBeGreaterThan(0);
    expect(summary.outputs).toBeLessThanOrEqual(summary.components);
    for (const id of summary.ids) {
      if (index.byId.get(id)!.kind === "Output") expect(summary.outputs).toBeGreaterThan(0);
    }
  });

  it("finds nothing downstream of a terminal Output", () => {
    expect(impact(index, "model.profit_margin").components).toBe(0);
  });
});

describe("focus mode", () => {
  it("keeps the seed and N hops each way, and nothing beyond", () => {
    const near = neighbourhood(index, "model.num_pols_if", 1, 1);
    expect(near.has("model.num_pols_if")).toBe(true);
    expect(near.has("model.qx")).toBe(true); // 1 up
    expect(near.has("model.deaths")).toBe(true); // 1 down
    expect(near.has("model.bel")).toBe(false); // 3 down
    const wider = neighbourhood(index, "model.num_pols_if", 2, 3);
    expect(wider.size).toBeGreaterThan(near.size);
  });
});

describe("filters", () => {
  it("treats an empty facet as no constraint", () => {
    expect(applyFilters(index, NO_FILTERS).size).toBe(TERM_ANNUAL.nodes.length);
  });

  it("intersects facets", () => {
    const outputs = applyFilters(index, { ...NO_FILTERS, kinds: ["Output"] });
    expect(outputs.size).toBe(13);
    const moneyOutputs = applyFilters(index, { ...NO_FILTERS, kinds: ["Output"], units: ["money"] });
    expect(moneyOutputs.size).toBeLessThan(outputs.size);
    for (const id of moneyOutputs) expect(index.byId.get(id)!.unit).toBe("money");
  });

  it("`affects X` keeps exactly the upstream closure of X, X included", () => {
    const kept = applyFilters(index, { ...NO_FILTERS, affects: "model.bel" });
    expect(kept.has("model.bel")).toBe(true);
    expect(kept.has("model.qx")).toBe(true);
    expect(kept.has("model.profit_margin")).toBe(false);
    expect(visibleEdges(TERM_ANNUAL, kept).every((e) => kept.has(e.from) && kept.has(e.to))).toBe(true);
  });

  it("offers the facet values the document actually contains", () => {
    const f = facets(TERM_ANNUAL);
    expect(f.modules).toEqual(expect.arrayContaining(["model", "schema"]));
    expect(f.units).toEqual(expect.arrayContaining(["money", "prob", "count"]));
    expect(f.kinds).toEqual(expect.arrayContaining(["Derived", "Output", "Table"]));
  });
});

describe("name resolution", () => {
  it("accepts a qualified id and a bare name", () => {
    expect(resolveId(index, "model.qx")).toBe("model.qx");
    expect(resolveId(index, "qx")).toBe("model.qx");
    expect(resolveId(index, "nonesuch")).toBeUndefined();
  });
});
