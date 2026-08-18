// Side-by-side modelpoint view (05-viz.md §4.2, last bullet).
//
// "Two column-aligned grids, differing cells highlighted, with both `explain()`
// traces stacked so the divergence point is visible — the first `t` where the
// trees disagree is auto-scrolled to and marked."
//
// Alignment is structural: the trace tree is the engine's own expression tree
// for the same component, so the two sides have the same shape wherever the
// models agree, and the first node whose *shape* differs is the edit itself.

import type { SeriesMatrix } from "../drilldown/grid";
import type { Trace, TraceNode } from "../drilldown/trace";

export interface AlignedRow {
  path: string;
  depth: number;
  a: TraceNode | null;
  b: TraceNode | null;
  /** Values differ beyond tolerance, or one side is missing this node. */
  differs: boolean;
  /** The node's shape differs (a different operator, ref, or arity). */
  structural: boolean;
  /**
   * The component whose expression this node belongs to — the nearest enclosing
   * `Component` node. A bare `Lit` two levels down means nothing on its own;
   * "a literal inside `premium_rate`" is a sentence about the model.
   */
  component: string | null;
  /** `both`, or the side a node is missing from. */
  presence: "both" | "only_a" | "only_b";
}

export interface TraceAlignment {
  rows: AlignedRow[];
  /** The topmost row where the two trees stop agreeing, or null. */
  firstDivergence: AlignedRow | null;
  /** The deepest structural difference on the path to `firstDivergence`. */
  firstStructural: AlignedRow | null;
}

/** Structural identity of a node, ignoring its value. */
function shapeOf(node: TraceNode): string {
  return [
    node.node,
    node.id ?? "",
    node.ref ?? "",
    node.op ?? "",
    node.fn ?? "",
    node.table ?? "",
    node.lag ?? "",
    node.at ?? "",
    node.resolution ?? "",
    node.children?.length ?? 0,
  ].join("|");
}

function close(a: number | undefined, b: number | undefined, tolerance: number): boolean {
  if (a === undefined || b === undefined) return a === b;
  if (Number.isNaN(a) && Number.isNaN(b)) return true;
  return Math.abs(a - b) <= tolerance;
}

/**
 * Walk both trees in the engine's child order, pairing nodes by position.
 *
 * Position, not path string: when one side has an extra factor the subtrees
 * below stop lining up, and that misalignment *is* the finding — it is reported
 * as a structural difference rather than smoothed over.
 */
export function alignTraces(a: Trace, b: Trace, tolerance = 0): TraceAlignment {
  const rows: AlignedRow[] = [];

  const walk = (
    left: TraceNode | null,
    right: TraceNode | null,
    depth: number,
    path: string,
    enclosing: string | null,
  ) => {
    const node = left ?? right;
    if (!node) return;
    const structural = !left || !right || shapeOf(left) !== shapeOf(right);
    const differs = structural || !close(left?.value, right?.value, tolerance);
    const component = left?.id ?? right?.id ?? enclosing;
    rows.push({
      path,
      depth,
      a: left,
      b: right,
      differs,
      structural,
      component,
      presence: left && right ? "both" : left ? "only_a" : "only_b",
    });
    const la = left?.children ?? [];
    const lb = right?.children ?? [];
    const n = Math.max(la.length, lb.length);
    for (let i = 0; i < n; i++) {
      walk(la[i] ?? null, lb[i] ?? null, depth + 1, `${path}.children[${i}]`, component);
    }
  };
  walk(a.root, b.root, 0, "root", null);

  // The *first* divergence in reading order is the shallowest one, which is
  // the number the reader already knows moved. The useful anchor is the
  // deepest node that still differs on an all-differing path from the root:
  // that is where the difference is introduced rather than propagated.
  const firstDivergence = rows.find((r) => r.differs) ?? null;
  let firstStructural: AlignedRow | null = null;
  for (const row of rows) {
    if (!row.structural) continue;
    if (!firstStructural || row.depth > firstStructural.depth) firstStructural = row;
  }
  return { rows, firstDivergence, firstStructural };
}

/** The sentence stacked above the two traces, or null when they agree. */
export function divergenceSentence(alignment: TraceAlignment): string | null {
  const row = alignment.firstStructural ?? alignment.firstDivergence;
  if (!row) return null;
  const where = row.component ? `inside \`${row.component}\`` : `at ${row.path}`;
  const label = (node: { node?: string; ref?: string; id?: string; op?: string } | null) =>
    node ? [node.node, node.id ?? node.ref ?? node.op].filter(Boolean).join(" ") : "nothing";
  if (row.presence !== "both") {
    const side = row.presence === "only_a" ? "A" : "B";
    return `the trees differ in shape ${where}: ${label(row.a ?? row.b)} is present only on side ${side}`;
  }
  if (row.structural) {
    return `the trees differ in shape ${where}: A has ${label(row.a)}, B has ${label(row.b)}`;
  }
  return `the trees agree in shape; the values first differ ${where}`;
}

export interface CellDelta {
  row: number;
  col: number;
  a: number | null;
  b: number | null;
  delta: number | null;
  differs: boolean;
}

export interface MatrixComparison {
  components: string[];
  periods: number[];
  cells: CellDelta[][];
  /** The first `t` at which any component differs — the auto-scroll target. */
  firstT: number | null;
  /** Component ids that differ somewhere. */
  differingComponents: string[];
  differingCells: number;
}

/**
 * Column-align the two grids on the union of components and periods, so a
 * component present on only one side shows as a row of empty cells rather than
 * silently shifting every column to its right.
 */
export function compareMatrices(
  a: SeriesMatrix,
  b: SeriesMatrix,
  tolerance = 0,
): MatrixComparison {
  const components = Array.from(new Set([...a.components, ...b.components]));
  const periods = Array.from(new Set([...a.periods, ...b.periods])).sort((x, y) => x - y);
  const pick = (m: SeriesMatrix, component: string, t: number): number | null => {
    const row = m.components.indexOf(component);
    const col = m.periods.indexOf(t);
    if (row === -1 || col === -1) return null;
    return m.values[row]?.[col] ?? null;
  };

  const cells: CellDelta[][] = [];
  const differingComponents: string[] = [];
  let firstT: number | null = null;
  let differingCells = 0;

  components.forEach((component, row) => {
    const line: CellDelta[] = [];
    let componentDiffers = false;
    periods.forEach((t, col) => {
      const va = pick(a, component, t);
      const vb = pick(b, component, t);
      const delta = va === null || vb === null ? null : vb - va;
      const differs =
        va === null || vb === null ? va !== vb : Math.abs(vb - va) > tolerance;
      if (differs) {
        differingCells += 1;
        componentDiffers = true;
        if (firstT === null || t < firstT) firstT = t;
      }
      line.push({ row, col, a: va, b: vb, delta, differs });
    });
    if (componentDiffers) differingComponents.push(component);
    cells.push(line);
  });

  return { components, periods, cells, firstT, differingComponents, differingCells };
}
