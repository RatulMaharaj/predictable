import { describe, expect, it } from "vitest";
import { buildWaterfall, timingGlyph, waterfallRows, yScale, type Flow } from "./waterfall";

const flows: Flow[] = [
  { component: "premium_income", value: 1317.3, timing: "start", declarationIndex: 0 },
  { component: "death_claims", value: -143.7, timing: "end", declarationIndex: 1 },
  { component: "renewal_expenses", value: -48.2, timing: "start", declarationIndex: 2 },
  { component: "interest", value: 43.6, timing: "mid", declarationIndex: 3 },
  { component: "reserve", value: 1284.55, timing: "point", kind: "Output", declarationIndex: 9 },
];

describe("buildWaterfall", () => {
  it("keeps declaration order — authorial intent, not magnitude", () => {
    const built = buildWaterfall(flows);
    expect(built.bars.map((b) => b.component)).toEqual([
      "premium_income",
      "death_claims",
      "renewal_expenses",
      "interest",
      "reserve",
    ]);
  });

  it("sorts by magnitude only when asked, and breaks ties by declaration", () => {
    const built = buildWaterfall(flows, { sortByMagnitude: true });
    expect(built.bars.map((b) => b.component)).toEqual([
      "premium_income",
      "death_claims",
      "renewal_expenses",
      "interest",
      "reserve",
    ]);
    const tied = buildWaterfall([
      { component: "b", value: -5, timing: "end", declarationIndex: 1 },
      { component: "a", value: 5, timing: "start", declarationIndex: 0 },
    ], { sortByMagnitude: true });
    expect(tied.bars.map((b) => b.component)).toEqual(["a", "b"]);
  });

  it("carries the running total on the connector, ending at the sum of the flows", () => {
    const built = buildWaterfall(flows);
    expect(built.bars[0].start).toBe(0);
    expect(built.bars[1].start).toBeCloseTo(1317.3, 10);
    expect(built.bars[3].end).toBeCloseTo(1169.0, 10);
    expect(built.total).toBeCloseTo(1169.0, 10);
  });

  it("draws the Output bar from zero, last, whatever its declaration index", () => {
    const built = buildWaterfall(flows);
    const last = built.bars[built.bars.length - 1];
    expect(last).toMatchObject({ component: "reserve", total: true, start: 0, end: 1284.55 });
  });

  it("flags mixed timings, because a summed timing mismatch is the classic bug", () => {
    expect(buildWaterfall(flows).mixedTimings).toBe(true);
    const uniform = buildWaterfall([
      { component: "a", value: 1, timing: "end", declarationIndex: 0 },
      { component: "b", value: 2, timing: "end", declarationIndex: 1 },
    ]);
    expect(uniform.mixedTimings).toBe(false);
  });

  it("gives every bar a timing glyph and a written timing label", () => {
    const built = buildWaterfall(flows);
    expect(built.bars.map((b) => b.timingLabel)).toEqual([
      "start",
      "end",
      "start",
      "mid",
      "point",
    ]);
    expect(new Set(built.bars.map((b) => b.glyph)).size).toBe(4);
    expect(timingGlyph(undefined)).toBe("·");
    const untimed = buildWaterfall([{ component: "x", value: 1, timing: undefined, declarationIndex: 0 }]);
    expect(untimed.bars[0].timingLabel).toBe("untimed");
  });

  it("spans the band the bars actually occupy, always including zero", () => {
    const built = buildWaterfall(flows);
    expect(built.min).toBe(0);
    expect(built.max).toBeCloseTo(1317.3, 10);
    const y = yScale(built, 200);
    expect(y(built.min)).toBe(200);
    expect(y(built.max)).toBe(0);
  });

  it("handles an empty waterfall without dividing by zero", () => {
    const built = buildWaterfall([]);
    expect(built.bars).toEqual([]);
    expect(yScale(built, 100)(0)).toBe(100);
  });

  it("exposes the same numbers as rows for the table equivalent", () => {
    const rows = waterfallRows(buildWaterfall(flows));
    expect(rows[0]).toEqual(["premium_income", "start", "1317.3", "1317.3"]);
    expect(rows).toHaveLength(5);
  });
});
