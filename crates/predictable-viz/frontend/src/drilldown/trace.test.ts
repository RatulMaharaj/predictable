import { describe, expect, it } from "vitest";
import reserveTrace from "./__fixtures__/trace-reserve.json";
import aggTrace from "./__fixtures__/trace-agg.json";
import {
  flattenTrace,
  formatDelta,
  formatExact,
  formatValue,
  nodeBadges,
  nodeLabel,
  notesByPath,
  spanLabel,
  type Trace,
  type TraceNode,
} from "./trace";

// The fixtures are real engine output: they were produced by
//   predictable explain models/term_annual/runs/base --component reserve --mp TA00001 --t 3 --json
// so these tests fail if the trace schema and this panel ever drift apart.
const reserve = reserveTrace as unknown as Trace;
const agg = aggTrace as unknown as Trace;

function nodes(node: TraceNode): TraceNode[] {
  return [node, ...(node.children ?? []).flatMap(nodes)];
}

describe("the engine's own trace", () => {
  it("parses as the documented envelope", () => {
    expect(reserve.format).toBe("pvf/1");
    expect(reserve.kind).toBe("trace");
    expect(reserve.root.node).toBe("Component");
    expect(reserve.root.id).toBe("model.reserve");
    expect(reserve.root.t).toBe(3);
    expect(reserve.root.modelpoint?.key).toBe("TA00001");
  });

  it("only uses node tags from the closed variant set of 04-verify §3.2", () => {
    const closed = new Set([
      "Component",
      "Binary",
      "Unary",
      "Ref",
      "Lag",
      "At",
      "Input",
      "Lit",
      "If",
      "Lookup",
      "Call",
      "Agg",
    ]);
    for (const node of nodes(reserve.root)) expect(closed).toContain(node.node);
  });

  it("flattens depth-first in the engine's child order, with every node once", () => {
    const rows = flattenTrace(reserve.root);
    const all = nodes(reserve.root);
    expect(rows).toHaveLength(all.length);
    expect(rows.map((r) => r.node.path)).toEqual(all.map((n) => n.path));
    expect(rows[0].depth).toBe(0);
    expect(new Set(rows.map((r) => r.node.path)).size).toBe(rows.length);
  });

  it("hides exactly the subtree under a collapsed path", () => {
    const branch = reserve.root.children![0];
    const hidden = nodes(branch).length - 1;
    const full = flattenTrace(reserve.root);
    const folded = flattenTrace(reserve.root, new Set([branch.path]));
    expect(full.length - folded.length).toBe(hidden);
    expect(folded.find((r) => r.node.path === branch.path)!.collapsed).toBe(true);
  });

  it("joins every note to a node that is actually in the tree", () => {
    const paths = new Set(nodes(reserve.root).map((n) => n.path));
    const byPath = notesByPath(reserve.notes);
    expect(reserve.notes!.length).toBeGreaterThan(0);
    for (const [path, list] of byPath) {
      expect(paths.has(path)).toBe(true);
      expect(list.every((n) => n.code.startsWith("N0"))).toBe(true);
    }
  });

  it("reports truncation rather than hiding it", () => {
    expect(reserve.truncated.elided_nodes).toBeGreaterThan(0);
    const elided = nodes(reserve.root).filter((n) => n.elided);
    expect(elided.length).toBeGreaterThan(0);
    expect(nodeBadges(elided[0]).map((b) => b.text)).toContainEqual(
      expect.stringContaining("children elided"),
    );
  });

  it("keeps the Agg contributions summing left to right to the value", () => {
    const node = nodes(agg.root).find((n) => n.node === "Agg")!;
    expect(node.terms!.length).toBe(node.over!.t_to - node.over!.t_from + 1);
    expect(node.terms!.map((t) => t.t)).toEqual(node.terms!.map((_, i) => i));
    let sum = 0;
    for (const term of node.terms!) sum += term.contribution;
    expect(sum).toBe(node.value);
  });
});

describe("labels and badges", () => {
  it("names each variant by its own identity", () => {
    expect(nodeLabel({ node: "Component", path: "root", value: 1, id: "m.reserve", t: 3 })).toBe(
      "m.reserve  t=3",
    );
    expect(nodeLabel({ node: "Lag", path: "p", value: 1, ref: "m.reserve", lag: 1, t: 2 })).toBe(
      "m.reserve[t-1]  t=2",
    );
    expect(nodeLabel({ node: "If", path: "p", value: 1, taken: "else" })).toBe("if → else");
    expect(
      nodeLabel({
        node: "Lookup",
        path: "p",
        value: 1,
        table: "sa8990",
        keys: [
          { name: "age", requested: "47", resolved: "47", policy: "exact", fired: false },
          { name: "sex", requested: "M", resolved: "M", policy: "exact", fired: false },
        ],
      }),
    ).toBe("sa8990[47, M]");
  });

  it("makes pre-origin resolution visible — the off-by-one bug class", () => {
    const init = nodeBadges({
      node: "Lag",
      path: "p",
      value: 842.1,
      t: -1,
      resolution: "init",
      init_expr: "bel",
    });
    expect(init[0].tone).toBe("warn");
    expect(init[0].text).toContain("used init = bel");

    const dflt = nodeBadges({
      node: "Lag",
      path: "p",
      value: 0,
      t: -1,
      resolution: "pre_origin_default",
    });
    expect(dflt[0].text).toContain("no init");
    expect(nodeBadges({ node: "Lag", path: "p", value: 1, t: 2, resolution: "computed" })).toEqual(
      [],
    );
  });

  it("warns only on the lookup keys whose policy actually fired", () => {
    const badges = nodeBadges({
      node: "Lookup",
      path: "p",
      value: 0.002,
      table: "sa8990",
      keys: [
        { name: "age", requested: "121", resolved: "120", policy: "clamp", fired: true },
        { name: "sex", requested: "M", resolved: "M", policy: "exact", fired: false },
      ],
    });
    expect(badges).toHaveLength(1);
    expect(badges[0]).toMatchObject({ tone: "warn", text: "age clamp 121 → 120" });
  });

  it("calls a retime a timing cast, not a value change", () => {
    const badges = nodeBadges({
      node: "Call",
      path: "p",
      value: 312.4,
      fn: "retime",
      from_timing: "start",
      to_timing: "mid",
    });
    expect(badges[0]).toMatchObject({ tone: "info", text: "timing start → mid, value unchanged" });
  });

  it("labels a span as file:line, and nothing when there is no span", () => {
    expect(
      spanLabel({ file: "models/term/model.pir", line: 148, col: 9, byte_start: 0, byte_end: 1 }),
    ).toBe("models/term/model.pir:148");
    expect(spanLabel(undefined)).toBeNull();
  });
});

describe("number formatting (§5.2)", () => {
  it("separates thousands and keeps the exact f64 available", () => {
    expect(formatValue(1284.5512345)).toBe("1,284.5512");
    expect(formatExact(1284.5512345)).toBe("1284.5512345");
    expect(formatExact(0.1 + 0.2)).toBe("0.30000000000000004");
  });

  it("never lets a small number read as zero", () => {
    expect(formatValue(0.00000214)).toBe("2.140000e-6");
    expect(formatValue(0)).toBe("0");
  });

  it("always signs a delta, with a glyph as well as a colour", () => {
    expect(formatDelta(1169)).toBe("+1,169");
    expect(formatDelta(-48.2)).toBe("−48.2");
    expect(formatDelta(0)).toBe("±0");
  });
});
