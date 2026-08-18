import { describe, expect, it } from "vitest";

import { fakeEngine } from "./fakeEngine";
import { fanExtent, runFan, scaleScenarios, toFan, tornadoAt, type Lineage } from "./fan";
import type { EngineInputs, SeriesColumns } from "./engine";

const inputs: EngineInputs = {
  sources: [{ name: "model.pir", text: "" }],
  assumptions: { mortality_loading: 1.0, lapse_loading: 1.0 },
  tables: [],
  modelpoints: "policy_number\nA\n",
};

const columns = (values: number[], mp = "A"): SeriesColumns => ({
  rows: values.length,
  mp: values.map(() => mp),
  t: values.map((_, i) => i),
  columns: { bel: values },
  model_digest: "sha256:m",
  plan_digest: "sha256:p",
  dropped: [],
});

const lineage = (label: string, to: number): Lineage => ({
  parent_run: "2026-06-30T00:00:00Z-base",
  parent_manifest_digest: "sha256:parent",
  group_id: "sens-mort",
  label,
  varied: [{ path: "assumptions.mortality_loading", from: 1, to }],
});

const response = {
  ok: true,
  group_id: "sens-mort",
  base: columns([100, 90, 80]),
  scenarios: [
    { lineage: lineage("mortality × 1.1", 1.1), series: columns([110, 99, 88]) },
    { lineage: lineage("mortality × 0.9", 0.9), series: columns([95, 86, 77]) },
  ],
  interpolated: false,
};

describe("scenario construction", () => {
  it("scales an assumption and never duplicates the base as a band", () => {
    const scenarios = scaleScenarios("mortality_loading", 1.05, [0.9, 1, 1.1]);
    expect(scenarios.map((s) => s.label)).toEqual([
      "mortality_loading × 0.9",
      "mortality_loading × 1.1",
    ]);
    expect(scenarios[1].set.mortality_loading).toBeCloseTo(1.155, 12);
  });
});

describe("the fan", () => {
  it("draws only computed points and says so in the data", () => {
    const fan = toFan(response, "bel", "A");
    expect(fan.interpolated).toBe(false);
    expect(fan.base.points).toEqual([
      { t: 0, value: 100 },
      { t: 1, value: 90 },
      { t: 2, value: 80 },
    ]);
    expect(fan.bands).toHaveLength(2);
    expect(fan.bands[0].points).toHaveLength(3);
  });

  it("keeps each band's Q12 lineage rather than re-deriving it", () => {
    const fan = toFan(response, "bel", "A");
    const band = fan.bands[0];
    expect(band.lineage).toEqual(lineage("mortality × 1.1", 1.1));
    expect(band.lineage!.group_id).toBe("sens-mort");
    expect(band.lineage!.varied[0]).toEqual({
      path: "assumptions.mortality_loading",
      from: 1,
      to: 1.1,
    });
    // A band is a real run and is inspectable: the parent is named, so the
    // diff screen can be handed this band as "run B".
    expect(band.lineage!.parent_run).toBe("2026-06-30T00:00:00Z-base");
  });

  it("leaves a gap where the run produced nothing, instead of a zero", () => {
    const holed = {
      ...response,
      base: {
        ...columns([100, Number.NaN, 80]),
      },
      scenarios: [],
    };
    const fan = toFan(holed, "bel", "A");
    expect(fan.base.points.map((p) => p.t)).toEqual([0, 2]);
  });

  it("selects the requested modelpoint out of a multi-lane projection", () => {
    const mixed: SeriesColumns = {
      rows: 4,
      mp: ["A", "A", "B", "B"],
      t: [0, 1, 0, 1],
      columns: { bel: [1, 2, 30, 40] },
      model_digest: "",
      plan_digest: "",
      dropped: [],
    };
    const fan = toFan({ ...response, base: mixed, scenarios: [] }, "bel", "B");
    expect(fan.base.points).toEqual([
      { t: 0, value: 30 },
      { t: 1, value: 40 },
    ]);
  });

  it("rejects a component the projection does not carry", () => {
    expect(() => toFan(response, "nope", "A")).toThrow(/not an output/);
  });

  it("shares one extent across the base and every band", () => {
    expect(fanExtent(toFan(response, "bel", "A"))).toEqual([77, 110]);
  });
});

describe("the tornado", () => {
  it("ranks by absolute impact at one t, with the lineage attached", () => {
    const bars = tornadoAt(toFan(response, "bel", "A"), 0);
    expect(bars.map((b) => b.label)).toEqual(["mortality × 1.1", "mortality × 0.9"]);
    expect(bars[0].delta).toBe(10);
    expect(bars[1].delta).toBe(-5);
    expect(bars[0].varied[0].path).toBe("assumptions.mortality_loading");
  });

  it("omits a band that computed nothing at t rather than drawing a zero bar", () => {
    const partial = {
      ...response,
      scenarios: [{ lineage: lineage("short", 1.1), series: columns([110]) }],
    };
    expect(tornadoAt(toFan(partial, "bel", "A"), 2)).toEqual([]);
    expect(tornadoAt(toFan(partial, "bel", "A"), 0)).toHaveLength(1);
  });

  it("is empty when the base itself has no point at t", () => {
    expect(tornadoAt(toFan(response, "bel", "A"), 99)).toEqual([]);
  });
});

describe("runFan", () => {
  it("asks the engine for a group and passes the parent through (Q12)", () => {
    const fake = fakeEngine(() => response);
    const fan = runFan(fake, inputs, {
      component: "bel",
      scenarios: scaleScenarios("mortality_loading", 1, [0.9, 1.1]),
      modelpoints: ["A"],
      groupId: "sens-mort",
      parentRun: "2026-06-30T00:00:00Z-base",
      parentManifestDigest: "sha256:parent",
    });
    const request = fake.requests[0];
    expect(request.op).toBe("sensitivity");
    expect(request.group_id).toBe("sens-mort");
    expect(request.parent_run).toBe("2026-06-30T00:00:00Z-base");
    expect((request.scenarios as unknown[]).length).toBe(2);
    expect(fan.groupId).toBe("sens-mort");
    expect(fan.bands).toHaveLength(2);
  });

  it("raises the engine's own failure rather than an empty chart", () => {
    const fake = fakeEngine(() => ({ ok: false, kind: "engine", error: "table `mortality` drifted" }));
    expect(() =>
      runFan(fake, inputs, { component: "bel", scenarios: [{ label: "x", set: {} }] }),
    ).toThrow(/drifted/);
  });
});
