// ELK layered layout — 05-viz.md §2.2.
//
// Three rules from the spec are implemented here and tested:
//
// 1. **Direction `RIGHT`, `layered`, `NETWORK_SIMPLEX`.** A dependency DAG with a
//    topological order must be read left to right.
// 2. **The visual layer index equals the evaluation depth.** The spec says
//    "`layering.strategy = LONGEST_PATH` seeded with the engine's `depth`".
//    ELK has no seed input for LONGEST_PATH, but it does have layer
//    *partitioning*: `elk.partitioning.partition = depth` pins a node to that
//    layer exactly. That is the stronger form of the same guarantee — the
//    picture cannot disagree with the plan — so it is what we ask for.
// 3. **Layout is cached, keyed by `model_digest`.** Unchanged model, identical
//    positions, across sessions: a screenshot in a review comment stays
//    meaningful.

import type { GraphDoc, GraphEdge, GraphNode } from "../datasource";

/** Node box size, in layout units. Derived from the label so long names fit. */
export function nodeSize(node: GraphNode): { width: number; height: number } {
  const label = node.name.length;
  return { width: Math.max(96, Math.min(260, 16 + label * 7.6)), height: 34 };
}

export interface ElkNode {
  id: string;
  width: number;
  height: number;
  layoutOptions: Record<string, string>;
}

export interface ElkEdge {
  id: string;
  sources: string[];
  targets: string[];
}

export interface ElkGraph {
  id: "root";
  layoutOptions: Record<string, string>;
  children: ElkNode[];
  edges: ElkEdge[];
}

export const LAYOUT_OPTIONS: Record<string, string> = {
  "elk.algorithm": "layered",
  "elk.direction": "RIGHT",
  "elk.layered.nodePlacement.strategy": "NETWORK_SIMPLEX",
  "elk.layered.layering.strategy": "LONGEST_PATH",
  // Pin the visual layer to the engine's `depth` (rule 2 above).
  "elk.partitioning.activate": "true",
  "elk.layered.spacing.nodeNodeBetweenLayers": "72",
  "elk.spacing.nodeNode": "18",
  "elk.layered.spacing.edgeNodeBetweenLayers": "18",
  "elk.edgeRouting": "ORTHOGONAL",
  "elk.layered.considerModelOrder.strategy": "NODES_AND_EDGES",
  // Without this, ELK lays every disconnected component out on its own and packs
  // the results side by side, which puts an unread input — a modelpoint field no
  // component reads yet, very common mid-migration — in a column to the right of
  // depth 2. The bands would then be lying about the evaluation order.
  "elk.separateConnectedComponents": "false",
};

/**
 * `GraphDoc` → the ELK input graph. Pure, and therefore the part that is
 * unit-tested; the worker only calls ELK on the result.
 *
 * Self edges (`x` reading `x[t-1]`) are dropped from the *layout* input — ELK
 * would otherwise reserve a whole layer for a loop that is drawn as a loop-back
 * arc on the node itself (§4.1: "the dashed `t-1` self-loop is drawn").
 */
export function toElkGraph(doc: GraphDoc): ElkGraph {
  const known = new Set(doc.nodes.map((n) => n.id));
  const children = doc.nodes.map((node) => ({
    id: node.id,
    ...nodeSize(node),
    layoutOptions: { "elk.partitioning.partition": String(node.depth) },
  }));
  const seen = new Set<string>();
  const edges: ElkEdge[] = [];
  for (const edge of doc.edges) {
    if (edge.from === edge.to) continue;
    if (!known.has(edge.from) || !known.has(edge.to)) continue;
    const id = `${edge.from}->${edge.to}`;
    if (seen.has(id)) continue;
    seen.add(id);
    edges.push({ id, sources: [edge.from], targets: [edge.to] });
  }
  return { id: "root", layoutOptions: LAYOUT_OPTIONS, children, edges };
}

export interface Placement {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface EdgeRoute {
  /** `from->to`; several `GraphEdge`s may share one route. */
  id: string;
  points: { x: number; y: number }[];
}

export interface Layout {
  model_digest: string;
  width: number;
  height: number;
  nodes: Record<string, Placement>;
  edges: Record<string, EdgeRoute>;
}

/** True when the layout covers every node in the document. */
export function layoutCovers(layout: Layout, doc: GraphDoc): boolean {
  return (
    layout.model_digest === doc.model_digest &&
    doc.nodes.every((n) => layout.nodes[n.id] !== undefined)
  );
}

/** The polyline an edge is drawn along, falling back to a straight run. */
export function routeFor(layout: Layout, edge: GraphEdge): { x: number; y: number }[] {
  const route = layout.edges[`${edge.from}->${edge.to}`];
  if (route && route.points.length >= 2) return route.points;
  const a = layout.nodes[edge.from];
  const b = layout.nodes[edge.to];
  if (!a || !b) return [];
  return [
    { x: a.x + a.width, y: a.y + a.height / 2 },
    { x: b.x, y: b.y + b.height / 2 },
  ];
}

// ---------------------------------------------------------------------------
// The cache
// ---------------------------------------------------------------------------

/**
 * The digest-keyed layout cache (§2.2).
 *
 * The spec writes it to `.predictable/viz-layout.json`; in a browser the same
 * contract is served by `localStorage`, which survives a reload and a new
 * session, and the static export carries no cache at all. The key is the
 * `model_digest` and nothing else — a changed model relayouts, an unchanged one
 * never does.
 */
export interface LayoutStore {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

export const CACHE_PREFIX = "predictable.viz.layout.";

export class LayoutCache {
  constructor(private readonly store: LayoutStore | undefined) {}

  static browser(): LayoutCache {
    try {
      return new LayoutCache(globalThis.localStorage);
    } catch {
      return new LayoutCache(undefined); // private mode, or no DOM
    }
  }

  get(digest: string): Layout | undefined {
    if (!this.store) return undefined;
    try {
      const raw = this.store.getItem(CACHE_PREFIX + digest);
      if (!raw) return undefined;
      const layout = JSON.parse(raw) as Layout;
      return layout.model_digest === digest ? layout : undefined;
    } catch {
      return undefined;
    }
  }

  put(layout: Layout): void {
    if (!this.store) return;
    try {
      this.store.setItem(CACHE_PREFIX + layout.model_digest, JSON.stringify(layout));
    } catch {
      // A quota failure must never break the screen; the layout is recomputable.
    }
  }

  drop(digest: string): void {
    this.store?.removeItem(CACHE_PREFIX + digest);
  }
}

/** One layout job, as [`crate::graph::useLayout::LayoutEngine`] receives it. */
export type LayoutRequest = { kind: "layout"; graph: ElkGraph; model_digest: string };

/** Convert ELK's output — the one impure boundary — into a [`Layout`]. */
export function fromElkResult(result: unknown, model_digest: string): Layout {
  const root = result as {
    width?: number;
    height?: number;
    children?: { id: string; x?: number; y?: number; width?: number; height?: number }[];
    edges?: {
      id: string;
      sections?: {
        startPoint: { x: number; y: number };
        bendPoints?: { x: number; y: number }[];
        endPoint: { x: number; y: number };
      }[];
    }[];
  };
  const nodes: Record<string, Placement> = {};
  for (const child of root.children ?? []) {
    nodes[child.id] = {
      x: child.x ?? 0,
      y: child.y ?? 0,
      width: child.width ?? 0,
      height: child.height ?? 0,
    };
  }
  const edges: Record<string, EdgeRoute> = {};
  for (const edge of root.edges ?? []) {
    const section = edge.sections?.[0];
    if (!section) continue;
    edges[edge.id] = {
      id: edge.id,
      points: [section.startPoint, ...(section.bendPoints ?? []), section.endPoint],
    };
  }
  return {
    model_digest,
    width: root.width ?? 0,
    height: root.height ?? 0,
    nodes,
    edges,
  };
}
