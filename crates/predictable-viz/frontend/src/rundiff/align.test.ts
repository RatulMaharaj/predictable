import { describe, expect, it } from "vitest";
import { matrixFromArrow } from "../drilldown/grid";
import type { Trace } from "../drilldown/trace";
import { alignTraces, compareMatrices, divergenceSentence } from "./align";
import { TRACE_A, TRACE_B, WALKTHROUGH_DIFF, WalkthroughSource } from "./__fixtures__";

const source = new WalkthroughSource();
const doc = WALKTHROUGH_DIFF;
const COMPONENTS = ["model.premium_income", "model.reserve", "model.bel"];

async function matrices(mp: string) {
  const [a, b] = await Promise.all([
    source.series({ run: doc.a.run, components: COMPONENTS, modelpoints: [mp] }),
    source.series({ run: doc.b.run, components: COMPONENTS, modelpoints: [mp] }),
  ]);
  return [matrixFromArrow(a, COMPONENTS), matrixFromArrow(b, COMPONENTS)] as const;
}

describe("side-by-side grids", () => {
  it("marks the first t at which the two runs disagree", async () => {
    const [a, b] = await matrices("TA00001");
    const cmp = compareMatrices(a, b, 0.005);
    expect(cmp.differingComponents).toContain("model.premium_income");
    expect(cmp.differingCells).toBeGreaterThan(0);

    // The marker is the earliest differing period across every row shown. On
    // this model `bel` carries a stage-2 value at t = -1, so it is earlier than
    // the diff's own headline first divergence — which is per component.
    const earliest = Math.min(
      ...cmp.cells.flat().filter((c) => c.differs).map((c) => cmp.periods[c.col]),
    );
    expect(cmp.firstT).toBe(earliest);

    const row = cmp.cells[cmp.components.indexOf("model.premium_income")];
    const firstForComponent = cmp.periods[row.find((c) => c.differs)!.col];
    expect(firstForComponent).toBe(doc.summary.first_divergence!.t);
    expect(doc.summary.first_divergence!.component).toBe("model.premium_income");
  });

  it("keeps the columns aligned when a component is missing on one side", async () => {
    const [a, b] = await matrices("TA00001");
    const trimmed = {
      components: b.components.slice(0, 1),
      periods: b.periods,
      values: b.values.slice(0, 1),
    };
    const cmp = compareMatrices(a, trimmed, 0.005);
    expect(cmp.components).toEqual(a.components);
    expect(cmp.periods).toEqual(a.periods);
    const missing = cmp.cells[cmp.components.indexOf("model.reserve")];
    expect(missing.every((cell) => cell.b === null)).toBe(true);
    expect(missing.every((cell) => cell.differs)).toBe(true);
  });

  it("reports no divergence when a run is compared with itself", async () => {
    const [a] = await matrices("TA00001");
    const cmp = compareMatrices(a, a, 0);
    expect(cmp.firstT).toBeNull();
    expect(cmp.differingCells).toBe(0);
  });
});

describe("trace alignment over the two real explain() traces", () => {
  const a = TRACE_A as Trace;
  const b = TRACE_B as Trace;

  it("finds the divergence at the root and localises it to the deepest shape change", () => {
    const alignment = alignTraces(a, b, 1e-9);
    expect(alignment.firstDivergence).not.toBeNull();
    expect(alignment.firstDivergence!.path).toBe("root");
    expect(alignment.firstDivergence!.a!.value).toBeCloseTo(151.39, 6);
    expect(alignment.firstDivergence!.b!.value).toBeCloseTo(155.9317, 4);

    // The seeded bug is in `premium_rate`'s `init`, so the trees stop having
    // the same shape inside the `premium_rate` subtree — not at `bel`.
    expect(alignment.firstStructural).not.toBeNull();
    const structural = alignment.firstStructural!;
    expect(structural.depth).toBeGreaterThan(alignment.firstDivergence!.depth);
    const inA = JSON.stringify(structural.a ?? {});
    const inB = JSON.stringify(structural.b ?? {});
    expect(inA === inB).toBe(false);
  });

  it("finds nothing when a trace is aligned with itself", () => {
    const alignment = alignTraces(a, a, 0);
    expect(alignment.firstDivergence).toBeNull();
    expect(alignment.firstStructural).toBeNull();
    expect(alignment.rows.length).toBeGreaterThan(5);
  });

  it("pairs nodes by position, so an extra factor shows as a structural difference", () => {
    const extra: Trace = {
      ...a,
      root: {
        ...a.root,
        children: [{ node: "Lit", path: "root.children[99]", value: 1.03 }, ...(a.root.children ?? [])],
      },
    };
    const alignment = alignTraces(a, extra, 0);
    expect(alignment.rows.some((r) => r.structural)).toBe(true);
    expect(alignment.firstDivergence!.structural).toBe(true);
  });
});

describe("the sentence above the traces", () => {
  it("names the component the shape difference sits inside", () => {
    const alignment = alignTraces(TRACE_A as Trace, TRACE_B as Trace, 1e-9);
    const sentence = divergenceSentence(alignment)!;
    // The seeded edit is `premium_rate`'s `init`, and the extra `1 +
    // premium_escalation` factor exists only on side B.
    expect(sentence).toContain("model.premium_rate");
    expect(sentence).toContain("only on side B");
    expect(alignment.firstStructural!.component).toBe("model.premium_rate");
    expect(alignment.firstStructural!.presence).toBe("only_b");
  });

  it("says nothing when the traces agree", () => {
    expect(divergenceSentence(alignTraces(TRACE_A as Trace, TRACE_A as Trace, 0))).toBeNull();
  });
});
