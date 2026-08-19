// The scene — what is drawn, before anyone decides *how* to draw it.
//
// The Canvas and SVG backends (§1.6) must be pixel-equivalent, so neither of them
// is allowed to decide anything: both consume this structure. That also makes the
// interesting behaviour — dimming in focus mode, amber for the impact set,
// filtered-out nodes hidden rather than removed — unit-testable without a DOM.

import type { GraphEdge, GraphNode } from "../datasource";
import { edgeStyle, isClipped, lintBadge, nodeShape, unitColour, type EdgeStyle, type NodeShape } from "./style";
import { applyFilters, impactSet, neighbourhood, type Filters, type GraphIndex } from "./model";
import { routeFor, type Layout } from "./layout";

export interface SceneNode {
  id: string;
  node: GraphNode;
  x: number;
  y: number;
  width: number;
  height: number;
  shape: NodeShape;
  clipped: boolean;
  colour: string;
  /** 1 for in focus, 0.08 for dimmed context (§2.3: dimmed, never removed). */
  opacity: number;
  selected: boolean;
  /** In the impact overlay's downstream closure. */
  impacted: boolean;
  lint?: { code: string; severity: string };
  /** `x` reads `x[t-k]`: drawn as a loop-back arc on the node itself (§4.1). */
  selfLag?: number;
}

export interface SceneEdge {
  edge: GraphEdge;
  style: EdgeStyle;
  points: { x: number; y: number }[];
  opacity: number;
  impacted: boolean;
}

export interface SceneBand {
  depth: number;
  x: number;
  width: number;
}

export interface Scene {
  nodes: SceneNode[];
  edges: SceneEdge[];
  /** Evaluation-depth bands, drawn behind everything when toggled on (§4.1). */
  bands: SceneBand[];
  width: number;
  height: number;
}

export interface SceneOptions {
  filters: Filters;
  selected?: string;
  /** Focus mode: dim everything outside N hops of the selection. */
  focus: boolean;
  focusUp?: number;
  focusDown?: number;
  /** Impact overlay: highlight the transitive downstream closure. */
  impact: boolean;
  /** Draw the evaluation-depth bands. */
  layers: boolean;
}

const DIM = 0.08;

export function buildScene(index: GraphIndex, layout: Layout, options: SceneOptions): Scene {
  const kept = applyFilters(index, options.filters);
  const focusIds =
    options.focus && options.selected
      ? neighbourhood(index, options.selected, options.focusUp ?? 2, options.focusDown ?? 2)
      : undefined;
  const impacted =
    options.impact && options.selected ? impactSet(index, options.selected) : new Set<string>();

  const selfLag = new Map<string, number>();
  for (const edge of index.doc.edges) {
    if (edge.from === edge.to && edge.lag !== null && edge.lag >= 1) {
      selfLag.set(edge.from, Math.max(selfLag.get(edge.from) ?? 0, edge.lag));
    }
  }

  const nodes: SceneNode[] = [];
  for (const node of index.doc.nodes) {
    if (!kept.has(node.id)) continue;
    const place = layout.nodes[node.id];
    if (!place) continue;
    nodes.push({
      id: node.id,
      node,
      ...place,
      shape: nodeShape(node),
      clipped: isClipped(node),
      colour: unitColour(node.unit),
      opacity: focusIds && !focusIds.has(node.id) ? DIM : 1,
      selected: node.id === options.selected,
      impacted: impacted.has(node.id),
      lint: lintBadge(node),
      selfLag: selfLag.get(node.id),
    });
  }

  const opacityOf = new Map(nodes.map((n) => [n.id, n.opacity]));
  const edges: SceneEdge[] = [];
  for (const edge of index.doc.edges) {
    if (edge.from === edge.to) continue; // drawn on the node as a loop-back arc
    if (!opacityOf.has(edge.from) || !opacityOf.has(edge.to)) continue;
    edges.push({
      edge,
      style: edgeStyle(edge),
      points: routeFor(layout, edge),
      opacity: Math.min(opacityOf.get(edge.from)!, opacityOf.get(edge.to)!),
      impacted: impacted.has(edge.to) && (impacted.has(edge.from) || edge.from === options.selected),
    });
  }

  const bands: SceneBand[] = [];
  if (options.layers) {
    const extent = new Map<number, { min: number; max: number }>();
    for (const n of nodes) {
      const current = extent.get(n.node.depth);
      if (!current) extent.set(n.node.depth, { min: n.x, max: n.x + n.width });
      else {
        current.min = Math.min(current.min, n.x);
        current.max = Math.max(current.max, n.x + n.width);
      }
    }
    for (const [depth, { min, max }] of Array.from(extent).sort((a, b) => a[0] - b[0])) {
      bands.push({ depth, x: min - 9, width: max - min + 18 });
    }
  }

  return { nodes, edges, bands, width: layout.width, height: layout.height };
}

/** Topmost node under a point — the shared hit test for both backends. */
export function hitTest(scene: Scene, x: number, y: number): SceneNode | undefined {
  for (let i = scene.nodes.length - 1; i >= 0; i -= 1) {
    const n = scene.nodes[i];
    if (n.opacity < 1) continue;
    if (x >= n.x && x <= n.x + n.width && y >= n.y && y <= n.y + n.height) return n;
  }
  return undefined;
}
