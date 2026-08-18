// The `Cmd-K` palette's index — 05-viz.md §2.3.
//
// > Typing a Prophet variable (`NUM_POLS_IF`) finds the predictable component
// > that claims it — this is how a migrating actuary navigates, and it is worth
// > more than any other feature in this screen.
//
// So Prophet variable names from `meta.source` are indexed as first-class search
// keys alongside component names, docs and tags, and a hit on one is *labelled*
// as Prophet provenance in the result row rather than silently renamed.

import type { GraphDoc, GraphNode } from "../datasource";

/** Which text of a component matched. Ordered by how much a hit on it means. */
export type MatchField = "name" | "prophet" | "tag" | "module" | "doc" | "expr";

const FIELD_WEIGHT: Record<MatchField, number> = {
  name: 100,
  prophet: 90,
  tag: 60,
  module: 40,
  doc: 30,
  expr: 20,
};

export interface SearchEntry {
  id: string;
  node: GraphNode;
  field: MatchField;
  /** The indexed text, as displayed. */
  text: string;
}

export interface SearchHit extends SearchEntry {
  score: number;
  /** Character positions in `text` that matched, for highlighting. */
  positions: number[];
}

/** Build the flat entry list. One node contributes several entries. */
export function buildSearchIndex(doc: GraphDoc): SearchEntry[] {
  const entries: SearchEntry[] = [];
  for (const node of doc.nodes) {
    entries.push({ id: node.id, node, field: "name", text: node.name });
    const variable = node.source?.variable;
    if (variable) {
      const library = node.source?.library;
      entries.push({
        id: node.id,
        node,
        field: "prophet",
        text: library ? `${library}.${variable}` : variable,
      });
    }
    for (const tag of node.tags) {
      entries.push({ id: node.id, node, field: "tag", text: tag });
    }
    if (node.module) entries.push({ id: node.id, node, field: "module", text: node.module });
    if (node.doc) entries.push({ id: node.id, node, field: "doc", text: node.doc });
    if (node.expr) entries.push({ id: node.id, node, field: "expr", text: node.expr });
  }
  return entries;
}

/**
 * Subsequence match, case-insensitive, scored so that (a) earlier matches beat
 * later ones, (b) contiguous runs beat scattered ones, (c) a match at a word
 * boundary — `_` and `.` included, because both `num_pols_if` and `NUM_POLS_IF`
 * are word-separated that way — beats one in the middle of a word.
 */
export function fuzzyMatch(query: string, text: string): { score: number; positions: number[] } | null {
  if (query.length === 0) return { score: 0, positions: [] };
  const q = query.toLowerCase();
  const t = text.toLowerCase();
  const positions: number[] = [];
  let score = 0;
  let ti = 0;
  let previous = -2;
  for (let qi = 0; qi < q.length; qi += 1) {
    const found = t.indexOf(q[qi], ti);
    if (found === -1) return null;
    positions.push(found);
    score += 10;
    if (found === previous + 1) score += 8; // contiguous
    const before = found === 0 ? "" : t[found - 1];
    if (found === 0 || before === "_" || before === "." || before === " ") score += 6;
    score -= Math.min(found, 20) * 0.1; // prefer early hits
    previous = found;
    ti = found + 1;
  }
  if (t === q) score += 40;
  else if (t.startsWith(q)) score += 20;
  return { score, positions };
}

/** Best hit per component, highest score first, then by plan order. */
export function search(entries: SearchEntry[], query: string, limit = 30): SearchHit[] {
  const trimmed = query.trim();
  const order = new Map<string, number>();
  for (const entry of entries) if (!order.has(entry.id)) order.set(entry.id, order.size);

  if (trimmed.length === 0) {
    return entries
      .filter((e) => e.field === "name")
      .slice(0, limit)
      .map((e) => ({ ...e, score: 0, positions: [] }));
  }

  const best = new Map<string, SearchHit>();
  for (const entry of entries) {
    const m = fuzzyMatch(trimmed, entry.text);
    if (!m) continue;
    const hit: SearchHit = {
      ...entry,
      score: m.score + FIELD_WEIGHT[entry.field],
      positions: m.positions,
    };
    const current = best.get(entry.id);
    if (!current || hit.score > current.score) best.set(entry.id, hit);
  }
  return Array.from(best.values())
    .sort((a, b) => b.score - a.score || order.get(a.id)! - order.get(b.id)!)
    .slice(0, limit);
}
