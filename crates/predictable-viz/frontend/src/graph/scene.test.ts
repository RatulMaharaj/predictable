import { describe, expect, it } from "vitest";
import { TERM_ANNUAL, withProphet } from "../test/fixtures";
import { indexGraph, NO_FILTERS } from "./model";
import { buildScene, hitTest } from "./scene";
import { paint, type Ctx2D } from "./canvas";
import { baseUnit, edgeStyle, nodeShape, rendererFor, unitColour, lintBadge, CANVAS_THRESHOLD } from "./style";
import type { Layout } from "./layout";

const index = indexGraph(TERM_ANNUAL);

/** A grid layout: one column per depth, so the scene tests need no ELK run. */
function gridLayout(doc = TERM_ANNUAL): Layout {
  const perDepth = new Map<number, number>();
  const nodes: Layout["nodes"] = {};
  for (const node of doc.nodes) {
    const row = perDepth.get(node.depth) ?? 0;
    perDepth.set(node.depth, row + 1);
    nodes[node.id] = { x: node.depth * 200, y: row * 50, width: 120, height: 34 };
  }
  return { model_digest: doc.model_digest, width: 2000, height: 1600, nodes, edges: {} };
}

const base = {
  filters: NO_FILTERS,
  focus: false,
  impact: false,
  layers: false,
};

describe("visual encoding of IR semantics (§2.2)", () => {
  it("gives inputs a pill, outputs the output style and tables their own shape", () => {
    const shapes = new Map(TERM_ANNUAL.nodes.map((n) => [n.id, nodeShape(n)]));
    expect(shapes.get("schema.entry_age")).toBe("pill");
    expect(shapes.get("<timeline>.t")).toBe("pill");
    expect(shapes.get("schema.mortality")).toBe("table");
    expect(shapes.get("model.bel")).toBe("output");
    expect(shapes.get("model.qx")).toBe("rect");
  });

  it("keeps a unit family's colour identity across bases", () => {
    expect(baseUnit("rate(annual)")).toBe("rate");
    expect(unitColour("rate(annual)")).toBe(unitColour("rate(monthly)"));
    expect(unitColour("money")).not.toBe(unitColour("prob"));
    expect(unitColour("no_such_unit")).toBe(unitColour("none"));
  });

  it("styles each edge kind distinctly, and never by colour alone", () => {
    const lag = TERM_ANNUAL.edges.find((e) => e.lag === 1 && e.from !== e.to)!;
    expect(edgeStyle(lag)).toMatchObject({ dash: "6 4", label: `t-${lag.lag}` });

    const same = TERM_ANNUAL.edges.find((e) => e.lag === 0 && e.via.startsWith("expr"))!;
    expect(edgeStyle(same)).toMatchObject({ dash: "", label: "", double: false });

    const table = TERM_ANNUAL.edges.find((e) => e.lag === null)!;
    expect(edgeStyle(table).kind).toBe("table");

    const stage2 = TERM_ANNUAL.edges.find((e) => e.stage === 2)!;
    expect(edgeStyle(stage2)).toMatchObject({ double: true, label: "⇒" });

    expect(
      edgeStyle({ ...same, at: true, lag: 3 }),
    ).toMatchObject({ dash: "1 3", label: "[3]" });

    // The one legal backward channel: an `init` read, drawn with a target dot.
    expect(edgeStyle({ ...same, via: "init.lhs" }).target_dot).toBe(true);
  });

  it("clips the corner of every stage-2 component", () => {
    const scene = buildScene(index, gridLayout(), base);
    const pv = scene.nodes.find((n) => n.id === "model.pv_claims")!;
    expect(pv.node.stage).toBe(2);
    expect(pv.clipped).toBe(true);
    expect(scene.nodes.find((n) => n.id === "model.qx")!.clipped).toBe(false);
  });

  it("surfaces the worst lint as the node's badge", () => {
    const scene = buildScene(indexGraph(withProphet()), gridLayout(withProphet()), base);
    const linted = scene.nodes.find((n) => n.id === "model.expense_scale")!;
    expect(linted.lint).toEqual({ code: "W0104", severity: "warn" });
    expect(scene.nodes.find((n) => n.id === "model.qx")!.lint).toBeUndefined();
    expect(lintBadge({ ...linted.node, lints: [] })).toBeUndefined();
  });
});

describe("the scene", () => {
  it("draws a self-referential component's `t-1` loop on the node, not as an edge", () => {
    const scene = buildScene(index, gridLayout(), base);
    expect(scene.nodes.find((n) => n.id === "model.num_pols_if")!.selfLag).toBe(1);
    expect(scene.edges.some((e) => e.edge.from === e.edge.to)).toBe(false);
  });

  it("dims context in focus mode rather than removing it (§2.3)", () => {
    const scene = buildScene(index, gridLayout(), {
      ...base,
      selected: "model.num_pols_if",
      focus: true,
    });
    expect(scene.nodes.length).toBe(TERM_ANNUAL.nodes.length); // nothing removed
    const dimmed = scene.nodes.filter((n) => n.opacity < 1);
    expect(dimmed.length).toBeGreaterThan(0);
    expect(dimmed.every((n) => n.opacity === 0.08)).toBe(true);
    expect(scene.nodes.find((n) => n.id === "model.num_pols_if")!.opacity).toBe(1);
    expect(scene.nodes.find((n) => n.id === "model.qx")!.opacity).toBe(1);
  });

  it("marks the downstream closure when the impact overlay is on", () => {
    const off = buildScene(index, gridLayout(), { ...base, selected: "model.qx" });
    expect(off.nodes.some((n) => n.impacted)).toBe(false);
    const on = buildScene(index, gridLayout(), { ...base, selected: "model.qx", impact: true });
    expect(on.nodes.find((n) => n.id === "model.deaths")!.impacted).toBe(true);
    expect(on.nodes.find((n) => n.id === "model.age")!.impacted).toBe(false);
    expect(on.edges.some((e) => e.impacted)).toBe(true);
  });

  it("hides filtered-out nodes and every edge touching them", () => {
    const scene = buildScene(index, gridLayout(), {
      ...base,
      filters: { ...NO_FILTERS, modules: ["model"] },
    });
    expect(scene.nodes.every((n) => n.node.module === "model")).toBe(true);
    const kept = new Set(scene.nodes.map((n) => n.id));
    expect(scene.edges.every((e) => kept.has(e.edge.from) && kept.has(e.edge.to))).toBe(true);
  });

  it("emits one band per occupied depth when layers are on", () => {
    expect(buildScene(index, gridLayout(), base).bands).toEqual([]);
    const banded = buildScene(index, gridLayout(), { ...base, layers: true });
    expect(banded.bands.map((b) => b.depth)).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8]);
    for (let i = 1; i < banded.bands.length; i += 1) {
      expect(banded.bands[i].x).toBeGreaterThan(banded.bands[i - 1].x);
    }
  });

  it("skips nodes the layout has no place for instead of drawing them at 0,0", () => {
    const layout = gridLayout();
    delete layout.nodes["model.qx"];
    const scene = buildScene(index, layout, base);
    expect(scene.nodes.some((n) => n.id === "model.qx")).toBe(false);
    expect(scene.edges.some((e) => e.edge.from === "model.qx")).toBe(false);
  });
});

describe("hit testing", () => {
  const scene = buildScene(index, gridLayout(), base);
  const target = scene.nodes.find((n) => n.id === "model.qx")!;

  it("picks the node under the point", () => {
    expect(hitTest(scene, target.x + 5, target.y + 5)!.id).toBe("model.qx");
    expect(hitTest(scene, target.x - 40, target.y + 5)).toBeUndefined();
  });

  it("ignores dimmed nodes, so focus mode's context is not clickable", () => {
    const focused = buildScene(index, gridLayout(), {
      ...base,
      selected: "model.num_pols_if",
      focus: true,
    });
    const far = focused.nodes.find((n) => n.opacity < 1)!;
    expect(hitTest(focused, far.x + 5, far.y + 5)).toBeUndefined();
  });
});

describe("the renderer split (§1.6)", () => {
  it("uses SVG below 300 visible nodes and Canvas above", () => {
    expect(rendererFor(48)).toBe("svg");
    expect(rendererFor(CANVAS_THRESHOLD)).toBe("svg");
    expect(rendererFor(CANVAS_THRESHOLD + 1)).toBe("canvas");
  });

  it("paints the Canvas scene in band → edge → node order", () => {
    const calls: string[] = [];
    const stub = new Proxy({} as Ctx2D, {
      get(_t, key: string) {
        if (["fillStyle", "strokeStyle", "lineWidth", "globalAlpha", "font", "textAlign", "textBaseline"].includes(key)) {
          return "";
        }
        return (...args: unknown[]) => {
          if (key === "fillText") calls.push(`text:${args[0]}`);
          else if (key === "fillRect") calls.push("rect");
          else if (key === "stroke") calls.push("stroke");
        };
      },
      set() {
        return true;
      },
    });
    const scene = buildScene(index, gridLayout(), { ...base, layers: true });
    paint(stub, scene, { zoom: 1, panX: 0, panY: 0, width: 800, height: 600 });

    // background + one band rect per depth, before any edge stroke.
    expect(calls.indexOf("rect")).toBe(0);
    expect(calls.filter((c) => c === "rect").length).toBe(1 + scene.bands.length);
    expect(calls.indexOf("stroke")).toBeLessThan(calls.findIndex((c) => c.startsWith("text:")));
    // every visible node gets its label drawn — the Canvas and SVG node sets match
    const labels = calls.filter((c) => c.startsWith("text:"));
    expect(labels.length).toBe(scene.nodes.length);
    expect(labels).toContain("text:num_pols_if");
  });
});
