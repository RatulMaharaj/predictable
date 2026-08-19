// The modelpoint grid's data model and its pure logic: matrix assembly from the
// Arrow batch, the virtualisation window, keyboard movement, and TSV copy.
//
// Nothing here touches the DOM, so all of it is testable without a browser and
// the component stays a thin renderer over these functions (05-viz.md §4.3).

import type { Table } from "apache-arrow";
import { formatExact, formatValue } from "./trace";

/** Components on rows, `t` on columns — the shape §3.4 level 3 asks for. */
export interface SeriesMatrix {
  /** Component names, in the order they were requested (declaration order). */
  components: string[];
  /** Ascending periods, one per column. */
  periods: number[];
  /** `values[row][col]`, `null` where the engine emitted nothing. */
  values: (number | null)[][];
}

/**
 * Build the matrix from the `/api/series` batch. The schema is fixed by
 * `source.rs`: `modelpoint: Utf8`, `t: UInt32`, then one `Float64` per
 * requested component in request order.
 */
export function matrixFromArrow(table: Table, components: string[]): SeriesMatrix {
  const tColumn = table.getChild("t");
  if (!tColumn) throw new Error("series batch has no `t` column");
  const periods: number[] = [];
  const seen = new Set<number>();
  for (let i = 0; i < table.numRows; i++) {
    const t = Number(tColumn.get(i));
    if (!seen.has(t)) {
      seen.add(t);
      periods.push(t);
    }
  }
  periods.sort((a, b) => a - b);
  const columnOf = new Map(periods.map((t, i) => [t, i]));
  const values = components.map(() => periods.map(() => null as number | null));
  components.forEach((name, row) => {
    const column = table.getChild(name);
    if (!column) return;
    for (let i = 0; i < table.numRows; i++) {
      const value = column.get(i);
      if (value === null || value === undefined) continue;
      values[row][columnOf.get(Number(tColumn.get(i)))!] = Number(value);
    }
  });
  return { components, periods, values };
}

/** A half-open window of rows and columns actually mounted in the DOM. */
export interface Window {
  rowStart: number;
  rowEnd: number;
  colStart: number;
  colEnd: number;
}

export interface Viewport {
  scrollTop: number;
  scrollLeft: number;
  height: number;
  width: number;
}

export interface Metrics {
  rowHeight: number;
  colWidth: number;
  /** Rows and columns rendered beyond the viewport so scrolling never tears. */
  overscan: number;
}

export const DEFAULT_METRICS: Metrics = { rowHeight: 24, colWidth: 96, overscan: 4 };

/**
 * The virtualisation window. 50,000 × 480 is the §5.3 target, so the grid
 * mounts a window, not a table: cost is bounded by the viewport, never by the
 * projection.
 */
export function visibleWindow(
  matrix: SeriesMatrix,
  viewport: Viewport,
  metrics: Metrics = DEFAULT_METRICS,
): Window {
  const { rowHeight, colWidth, overscan } = metrics;
  const rows = matrix.components.length;
  const cols = matrix.periods.length;
  const clamp = (v: number, hi: number) => Math.max(0, Math.min(hi, v));
  const rowStart = clamp(Math.floor(viewport.scrollTop / rowHeight) - overscan, rows);
  const rowEnd = clamp(Math.ceil((viewport.scrollTop + viewport.height) / rowHeight) + overscan, rows);
  const colStart = clamp(Math.floor(viewport.scrollLeft / colWidth) - overscan, cols);
  const colEnd = clamp(Math.ceil((viewport.scrollLeft + viewport.width) / colWidth) + overscan, cols);
  return { rowStart, rowEnd, colStart, colEnd };
}

export interface Cell {
  row: number;
  col: number;
}

/** An inclusive rectangle: the anchor cell plus wherever shift-arrow reached. */
export interface Selection {
  anchor: Cell;
  focus: Cell;
}

export function selectionBounds(selection: Selection) {
  return {
    rowStart: Math.min(selection.anchor.row, selection.focus.row),
    rowEnd: Math.max(selection.anchor.row, selection.focus.row),
    colStart: Math.min(selection.anchor.col, selection.focus.col),
    colEnd: Math.max(selection.anchor.col, selection.focus.col),
  };
}

export function isSelected(selection: Selection, row: number, col: number): boolean {
  const b = selectionBounds(selection);
  return row >= b.rowStart && row <= b.rowEnd && col >= b.colStart && col <= b.colEnd;
}

/**
 * Keyboard movement (§5.2: full navigation without a mouse). Returns the new
 * selection, or `null` when the key is not ours to handle.
 */
export function moveSelection(
  matrix: SeriesMatrix,
  selection: Selection,
  key: string,
  extend: boolean,
): Selection | null {
  const lastRow = matrix.components.length - 1;
  const lastCol = matrix.periods.length - 1;
  const { row, col } = selection.focus;
  let next: Cell;
  switch (key) {
    case "ArrowUp":
      next = { row: Math.max(0, row - 1), col };
      break;
    case "ArrowDown":
      next = { row: Math.min(lastRow, row + 1), col };
      break;
    case "ArrowLeft":
      next = { row, col: Math.max(0, col - 1) };
      break;
    case "ArrowRight":
      next = { row, col: Math.min(lastCol, col + 1) };
      break;
    case "Home":
      next = { row, col: 0 };
      break;
    case "End":
      next = { row, col: lastCol };
      break;
    case "PageUp":
      next = { row: 0, col };
      break;
    case "PageDown":
      next = { row: lastRow, col };
      break;
    default:
      return null;
  }
  return extend ? { anchor: selection.anchor, focus: next } : { anchor: next, focus: next };
}

/**
 * TSV for the clipboard. Excel splits on tabs and newlines and nothing else, so
 * the header carries `component` then one `t=` label per column, and every
 * number is the engine's full `f64` — a copy that rounds is a copy that lies.
 * Empty cells stay empty rather than becoming `0`.
 */
export function selectionToTsv(matrix: SeriesMatrix, selection: Selection): string {
  const b = selectionBounds(selection);
  const header = ["component"];
  for (let c = b.colStart; c <= b.colEnd; c++) header.push(`t=${matrix.periods[c]}`);
  const lines = [header.join("\t")];
  for (let r = b.rowStart; r <= b.rowEnd; r++) {
    const cells = [matrix.components[r]];
    for (let c = b.colStart; c <= b.colEnd; c++) {
      const value = matrix.values[r][c];
      cells.push(value === null ? "" : formatExact(value));
    }
    lines.push(cells.join("\t"));
  }
  return lines.join("\n");
}

/** The em-dash the mock uses for a period the component has no value at. */
export const EMPTY_CELL = "—";

export function cellText(value: number | null): string {
  return value === null ? EMPTY_CELL : formatValue(value);
}

/**
 * Heat overlay intensity in `[0, 1]`, scaled per row: a row is one component in
 * one unit, and scaling across units would compare money with probabilities.
 * Returns `null` for an empty cell or a flat row (no heat rather than false
 * heat).
 */
export function heatIntensity(matrix: SeriesMatrix, row: number, col: number): number | null {
  const value = matrix.values[row][col];
  if (value === null) return null;
  let lo = Infinity;
  let hi = -Infinity;
  for (const v of matrix.values[row]) {
    if (v === null) continue;
    lo = Math.min(lo, v);
    hi = Math.max(hi, v);
  }
  if (!Number.isFinite(lo) || hi === lo) return null;
  return (value - lo) / (hi - lo);
}
