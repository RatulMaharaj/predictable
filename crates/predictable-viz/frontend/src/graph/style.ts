// Visual encoding — 05-viz.md §2.2 and §5.2.
//
// Every rule here encodes IR semantics and nothing else, and none of them uses
// colour as the *only* channel: kind also changes the node outline, lag also
// changes the dash pattern and carries a text label, stage also changes the node
// corner. The palette is checked against deuteranopia/protanopia.

import type { GraphEdge, GraphNode } from "../datasource";

/** Hue per `Unit`, shared with the results charts so identity carries over. */
export const UNIT_COLOURS: Record<string, string> = {
  money: "#3d7dd6",
  prob: "#b3562f",
  rate: "#7a5bb5",
  count: "#2f8a6a",
  factor: "#a3781f",
  time: "#4a6b8a",
  none: "#6b7280",
};

/**
 * `rate(annual)` and `rate(monthly)` are the same unit family and must keep the
 * same colour identity across screens, so the basis in parentheses is stripped
 * before the lookup.
 */
export function baseUnit(unit: string): string {
  const paren = unit.indexOf("(");
  return paren === -1 ? unit : unit.slice(0, paren);
}

export function unitColour(unit: string): string {
  return UNIT_COLOURS[baseUnit(unit)] ?? UNIT_COLOURS.none;
}

/** Node geometry class. `Table` nodes are rounded rects with a distinct fill. */
export type NodeShape = "pill" | "rect" | "output" | "table";

export function nodeShape(node: GraphNode): NodeShape {
  if (node.kind === "Table") return "table";
  if (node.kind.startsWith("Input")) return "pill";
  if (node.kind === "Output") return "output";
  return "rect";
}

/** `stage = 2` gets a clipped corner regardless of kind (§2.2). */
export function isClipped(node: GraphNode): boolean {
  return node.stage === 2;
}

export interface EdgeStyle {
  /** SVG `stroke-dasharray`; empty string means solid. */
  dash: string;
  width: number;
  /** Midpoint label: `t-k`, `[k]`, `⇒`, or none. */
  label: string;
  /** Two parallel strokes for a stage-2 whole-series edge. */
  double: boolean;
  /** A small circle at the target: the legal `init` → stage-2 back-channel. */
  target_dot: boolean;
  kind: "same-period" | "lag" | "at" | "stage2" | "table";
}

/**
 * The §2.2 table, in code.
 *
 * `via` decides the `init` back-channel case: a reference whose path starts with
 * `init` is the one legal backward read (IR §8.2), and it is drawn distinctly
 * *because* it is the exception a reader must be able to spot.
 */
export function edgeStyle(edge: GraphEdge): EdgeStyle {
  const fromInit = edge.via.startsWith("init");
  if (edge.lag === null) {
    return {
      dash: "2 4",
      width: 1,
      label: "",
      double: false,
      target_dot: false,
      kind: "table",
    };
  }
  if (edge.at) {
    return {
      dash: "1 3",
      width: 1,
      label: `[${edge.lag}]`,
      double: false,
      target_dot: fromInit,
      kind: "at",
    };
  }
  if (edge.stage === 2) {
    return {
      dash: "",
      width: 1.5,
      label: "⇒",
      double: true,
      target_dot: fromInit,
      kind: "stage2",
    };
  }
  if (edge.lag >= 1) {
    return {
      dash: "6 4",
      width: 1,
      label: `t-${edge.lag}`,
      double: false,
      target_dot: fromInit,
      kind: "lag",
    };
  }
  return {
    dash: fromInit ? "2 3" : "",
    width: 1,
    label: "",
    double: false,
    target_dot: fromInit,
    kind: "same-period",
  };
}

/** Above this many *visible* nodes the renderer switches to Canvas (§1.6). */
export const CANVAS_THRESHOLD = 300;

export type RendererKind = "svg" | "canvas";

export function rendererFor(visibleNodes: number): RendererKind {
  return visibleNodes > CANVAS_THRESHOLD ? "canvas" : "svg";
}

/** Highest-severity lint on a node, for the warning triangle (§4.1). */
export function lintBadge(node: GraphNode): { code: string; severity: string } | undefined {
  const lints = node.lints ?? [];
  if (lints.length === 0) return undefined;
  const rank = (s: string) => (s === "error" ? 0 : s === "warn" ? 1 : 2);
  const worst = [...lints].sort((a, b) => rank(a.severity) - rank(b.severity))[0];
  return { code: worst.code, severity: worst.severity };
}
