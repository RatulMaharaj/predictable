// Derived views over a `GraphDoc` — 05-viz.md §2.1, §2.3.
//
// Everything in this file is a *pure function of the document the engine sent*.
// No topological order is computed here: `doc.layers` is the planner's own
// ordering and every layer index used downstream comes from `node.depth`.

import type { GraphDoc, GraphEdge, GraphNode } from "../datasource";

/** Adjacency in both directions, built once per document. */
export interface GraphIndex {
  doc: GraphDoc;
  byId: Map<string, GraphNode>;
  /** id → ids it reads. */
  upstream: Map<string, string[]>;
  /** id → ids that read it. */
  downstream: Map<string, string[]>;
}

function push(map: Map<string, string[]>, key: string, value: string): void {
  const list = map.get(key);
  if (!list) map.set(key, [value]);
  else if (!list.includes(value)) list.push(value);
}

export function indexGraph(doc: GraphDoc): GraphIndex {
  const byId = new Map<string, GraphNode>();
  for (const node of doc.nodes) byId.set(node.id, node);
  const upstream = new Map<string, string[]>();
  const downstream = new Map<string, string[]>();
  for (const edge of doc.edges) {
    push(upstream, edge.to, edge.from);
    push(downstream, edge.from, edge.to);
  }
  return { doc, byId, upstream, downstream };
}

/** Resolve a bare name or a qualified id to a node id. */
export function resolveId(index: GraphIndex, name: string): string | undefined {
  if (index.byId.has(name)) return name;
  return index.doc.nodes.find((n) => n.name === name)?.id;
}

/**
 * The transitive downstream closure of `seed` — the impact overlay (§2.3).
 *
 * Self-referential components (`x[t-1]` inside `x`) produce a self edge, and a
 * component is never reported as impacting itself, so the count an actuary reads
 * is "how many *other* components move".
 */
export function impactSet(index: GraphIndex, seed: string): Set<string> {
  const seen = new Set<string>();
  const queue = [seed];
  while (queue.length) {
    const id = queue.pop()!;
    for (const next of index.downstream.get(id) ?? []) {
      if (next === seed || seen.has(next)) continue;
      seen.add(next);
      queue.push(next);
    }
  }
  return seen;
}

/** Impact summary as the sidebar phrases it: "…affects 14 components, 4 Outputs". */
export interface Impact {
  seed: string;
  ids: Set<string>;
  components: number;
  outputs: number;
}

export function impact(index: GraphIndex, seed: string): Impact {
  const ids = impactSet(index, seed);
  let outputs = 0;
  for (const id of ids) if (index.byId.get(id)?.kind === "Output") outputs += 1;
  return { seed, ids, components: ids.size, outputs };
}

/** The focus-mode neighbourhood: `up` hops upstream, `down` hops downstream (§2.3). */
export function neighbourhood(
  index: GraphIndex,
  seed: string,
  up = 2,
  down = 2,
): Set<string> {
  const keep = new Set<string>([seed]);
  const walk = (dir: Map<string, string[]>, hops: number) => {
    let frontier = [seed];
    for (let h = 0; h < hops; h += 1) {
      const next: string[] = [];
      for (const id of frontier) {
        for (const other of dir.get(id) ?? []) {
          if (keep.has(other)) continue;
          keep.add(other);
          next.push(other);
        }
      }
      frontier = next;
    }
  };
  walk(index.upstream, up);
  walk(index.downstream, down);
  return keep;
}

/** Filter predicate state, mirrored in the URL (§5.1). */
export interface Filters {
  modules: string[];
  units: string[];
  kinds: string[];
  tags: string[];
  /** Keep only the upstream closure of this output, when set. */
  affects?: string;
}

export const NO_FILTERS: Filters = { modules: [], units: [], kinds: [], tags: [] };

export function filtersActive(f: Filters): boolean {
  return (
    f.modules.length > 0 ||
    f.units.length > 0 ||
    f.kinds.length > 0 ||
    f.tags.length > 0 ||
    f.affects !== undefined
  );
}

function upstreamClosure(index: GraphIndex, seed: string): Set<string> {
  const seen = new Set<string>([seed]);
  const queue = [seed];
  while (queue.length) {
    const id = queue.pop()!;
    for (const prev of index.upstream.get(id) ?? []) {
      if (seen.has(prev)) continue;
      seen.add(prev);
      queue.push(prev);
    }
  }
  return seen;
}

/** Ids surviving the filter set. An empty facet means "no constraint". */
export function applyFilters(index: GraphIndex, f: Filters): Set<string> {
  const affects = f.affects ? upstreamClosure(index, f.affects) : undefined;
  const keep = new Set<string>();
  for (const node of index.doc.nodes) {
    if (f.modules.length && !f.modules.includes(node.module)) continue;
    if (f.units.length && !f.units.includes(node.unit)) continue;
    if (f.kinds.length && !f.kinds.includes(node.kind)) continue;
    if (f.tags.length && !f.tags.some((t) => node.tags.includes(t))) continue;
    if (affects && !affects.has(node.id)) continue;
    keep.add(node.id);
  }
  return keep;
}

/** Distinct facet values, in the document's own (plan) order. */
export function facets(doc: GraphDoc): {
  modules: string[];
  units: string[];
  kinds: string[];
  tags: string[];
} {
  const uniq = (xs: string[]) => Array.from(new Set(xs.filter((x) => x.length > 0)));
  return {
    modules: uniq(doc.nodes.map((n) => n.module)),
    units: uniq(doc.nodes.map((n) => n.unit)),
    kinds: uniq(doc.nodes.map((n) => n.kind)),
    tags: uniq(doc.nodes.flatMap((n) => n.tags)),
  };
}

/** Every edge between two kept nodes, plus self edges. */
export function visibleEdges(doc: GraphDoc, kept: Set<string>): GraphEdge[] {
  return doc.edges.filter((e) => kept.has(e.from) && kept.has(e.to));
}
