import { describe, expect, it } from "vitest";
import { tableFromArrays } from "apache-arrow";
import {
  cellText,
  heatIntensity,
  isSelected,
  matrixFromArrow,
  moveSelection,
  selectionToTsv,
  visibleWindow,
  type SeriesMatrix,
} from "./grid";

const matrix: SeriesMatrix = {
  components: ["num_pols_if", "premium_income", "death_claims", "reserve"],
  periods: [0, 1, 2, 3],
  values: [
    [1, 0.9721, 0.9447, 0.9178],
    [1240, 1265.7, 1291.4, 1317.3],
    [null, 134.2, 138.9, 143.7],
    [1084.2, 1131.8, 1180.1, 1284.5512345],
  ],
};

const cell = (row: number, col: number) => ({ anchor: { row, col }, focus: { row, col } });

describe("matrixFromArrow", () => {
  it("builds components × t from the /api/series schema", () => {
    const table = tableFromArrays({
      modelpoint: ["TA00001", "TA00001", "TA00001"],
      t: Uint32Array.from([0, 1, 2]),
      reserve: Float64Array.from([1084.2, 1131.8, 1180.1]),
      num_pols_if: Float64Array.from([1, 0.9721, 0.9447]),
    });
    const built = matrixFromArrow(table, ["reserve", "num_pols_if"]);
    expect(built.components).toEqual(["reserve", "num_pols_if"]);
    expect(built.periods).toEqual([0, 1, 2]);
    expect(built.values[0]).toEqual([1084.2, 1131.8, 1180.1]);
    expect(built.values[1][1]).toBeCloseTo(0.9721, 12);
  });

  it("keeps rows for a component the batch did not carry, rather than shifting them", () => {
    const table = tableFromArrays({
      modelpoint: ["TA00001"],
      t: Uint32Array.from([0]),
      reserve: Float64Array.from([1084.2]),
    });
    const built = matrixFromArrow(table, ["reserve", "not_emitted"]);
    expect(built.values).toEqual([[1084.2], [null]]);
  });

  it("refuses a batch with no `t` column instead of guessing", () => {
    const table = tableFromArrays({ modelpoint: ["TA00001"] });
    expect(() => matrixFromArrow(table, [])).toThrow(/no `t` column/);
  });
});

describe("virtualisation", () => {
  const big: SeriesMatrix = {
    components: Array.from({ length: 5000 }, (_, i) => `c${i}`),
    periods: Array.from({ length: 480 }, (_, i) => i),
    values: [],
  };

  it("mounts a window bounded by the viewport, not by the projection", () => {
    const w = visibleWindow(big, { scrollTop: 2400, scrollLeft: 960, height: 480, width: 960 });
    expect(w.rowStart).toBe(96); // 2400/24 - 4 overscan
    expect(w.rowEnd).toBe(124); // + 480/24 + 4
    expect(w.colStart).toBe(6);
    expect(w.colEnd).toBe(24);
    expect(w.rowEnd - w.rowStart).toBeLessThan(40);
  });

  it("clamps at both ends so no phantom rows are mounted", () => {
    const top = visibleWindow(big, { scrollTop: 0, scrollLeft: 0, height: 480, width: 960 });
    expect(top.rowStart).toBe(0);
    expect(top.colStart).toBe(0);
    const bottom = visibleWindow(big, {
      scrollTop: 5000 * 24,
      scrollLeft: 480 * 96,
      height: 480,
      width: 960,
    });
    expect(bottom.rowEnd).toBe(5000);
    expect(bottom.colEnd).toBe(480);
  });
});

describe("selection and keyboard navigation", () => {
  it("moves the focus and collapses the selection unless shift extends it", () => {
    expect(moveSelection(matrix, cell(1, 1), "ArrowDown", false)).toEqual(cell(2, 1));
    expect(moveSelection(matrix, cell(1, 1), "ArrowRight", true)).toEqual({
      anchor: { row: 1, col: 1 },
      focus: { row: 1, col: 2 },
    });
  });

  it("stops at the edges", () => {
    expect(moveSelection(matrix, cell(0, 0), "ArrowUp", false)).toEqual(cell(0, 0));
    expect(moveSelection(matrix, cell(3, 3), "ArrowRight", false)).toEqual(cell(3, 3));
    expect(moveSelection(matrix, cell(1, 1), "End", false)).toEqual(cell(1, 3));
    expect(moveSelection(matrix, cell(1, 1), "PageDown", false)).toEqual(cell(3, 1));
  });

  it("leaves keys it does not own to the browser", () => {
    expect(moveSelection(matrix, cell(1, 1), "a", false)).toBeNull();
  });

  it("selects the rectangle between anchor and focus", () => {
    const rect = { anchor: { row: 0, col: 1 }, focus: { row: 2, col: 3 } };
    expect(isSelected(rect, 1, 2)).toBe(true);
    expect(isSelected(rect, 3, 2)).toBe(false);
    expect(isSelected(rect, 1, 0)).toBe(false);
  });
});

describe("TSV copy", () => {
  it("copies the full f64, tab separated, with a t header Excel understands", () => {
    const tsv = selectionToTsv(matrix, { anchor: { row: 1, col: 1 }, focus: { row: 3, col: 2 } });
    expect(tsv.split("\n")).toEqual([
      "component\tt=1\tt=2",
      "premium_income\t1265.7\t1291.4",
      "death_claims\t134.2\t138.9",
      "reserve\t1131.8\t1180.1",
    ]);
  });

  it("does not round — a copy that rounds is a copy that lies", () => {
    const tsv = selectionToTsv(matrix, cell(3, 3));
    expect(tsv).toBe("component\tt=3\nreserve\t1284.5512345");
  });

  it("copies an absent value as empty, never as zero", () => {
    const tsv = selectionToTsv(matrix, cell(2, 0));
    expect(tsv).toBe("component\tt=0\ndeath_claims\t");
    expect(cellText(null)).toBe("—");
  });

  it("normalises an inverted selection", () => {
    const up = selectionToTsv(matrix, { anchor: { row: 3, col: 2 }, focus: { row: 1, col: 1 } });
    const down = selectionToTsv(matrix, { anchor: { row: 1, col: 1 }, focus: { row: 3, col: 2 } });
    expect(up).toBe(down);
  });
});

describe("heat overlay", () => {
  it("scales per row, because a row is one component in one unit", () => {
    expect(heatIntensity(matrix, 0, 0)).toBe(1);
    expect(heatIntensity(matrix, 0, 3)).toBe(0);
    expect(heatIntensity(matrix, 3, 3)).toBe(1);
  });

  it("gives no heat rather than false heat", () => {
    expect(heatIntensity(matrix, 2, 0)).toBeNull();
    const flat: SeriesMatrix = { components: ["c"], periods: [0, 1], values: [[2, 2]] };
    expect(heatIntensity(flat, 0, 0)).toBeNull();
  });
});
