// Contributor bars (05-viz.md §4.2, right panel).
//
// The diff document ranks *findings*; it does not carry every diverging cell —
// it carries an exemplar and a worst case (04-verify §5.4). The per-modelpoint
// ranking §4.2 draws is therefore computed here, in the client, from the two
// runs' own series. Under §0's rule that makes it a **derived quantity**, and
// every function below carries the method string that says how it was derived.
// The screen prints that string; an unstated method is a lie.

import type { Table } from "apache-arrow";

/** One modelpoint's movement in one component, summed over the periods held. */
export interface Contributor {
  mp: string;
  /** Σ(b − a) over `t`. Signed: the sign is the direction of the movement. */
  delta: number;
  /** Σ|b − a|, which is what the bar length is proportional to. */
  absDelta: number;
  /** `absDelta` as a fraction of the total absolute movement, 0…1. */
  share: number;
  /** Periods where |b − a| exceeded `tolerance`. */
  cells: number;
  /** The earliest such period, or null when this modelpoint matches. */
  tFirst: number | null;
}

export interface ContributorRanking {
  component: string;
  contributors: Contributor[];
  /** Modelpoint keys with no cell outside tolerance. */
  matching: string[];
  /** Modelpoint keys with at least one. */
  differing: string[];
  totalAbs: number;
  totalDelta: number;
  /** Rendered verbatim next to the bars. */
  method: string;
}

/** The `/api/series` batch, keyed the way `source.rs` fixes the schema. */
export interface SeriesFrame {
  mp: string[];
  t: number[];
  values: (number | null)[];
}

/**
 * Pull one component's column out of a series batch.
 *
 * Schema (`source.rs`): `modelpoint: Utf8`, `t: UInt32`, then one `Float64` per
 * requested component in request order.
 */
export function frameOf(table: Table, component: string): SeriesFrame {
  const mpColumn = table.getChild("modelpoint");
  const tColumn = table.getChild("t");
  if (!mpColumn) throw new Error("series batch has no `modelpoint` column");
  if (!tColumn) throw new Error("series batch has no `t` column");
  const column = table.getChild(component);
  if (!column) throw new Error(`series batch has no column for \`${component}\``);
  const mp: string[] = [];
  const t: number[] = [];
  const values: (number | null)[] = [];
  for (let i = 0; i < table.numRows; i++) {
    mp.push(String(mpColumn.get(i)));
    t.push(Number(tColumn.get(i)));
    const v = column.get(i);
    values.push(v === null || v === undefined ? null : Number(v));
  }
  return { mp, t, values };
}

function keyed(frame: SeriesFrame): Map<string, Map<number, number | null>> {
  const out = new Map<string, Map<number, number | null>>();
  for (let i = 0; i < frame.mp.length; i++) {
    let row = out.get(frame.mp[i]);
    if (!row) {
      row = new Map();
      out.set(frame.mp[i], row);
    }
    row.set(frame.t[i], frame.values[i]);
  }
  return out;
}

/**
 * Rank modelpoints by how much of one component's movement they carry.
 *
 * `tolerance` is the absolute tolerance the diff itself applied to this
 * component (`doc.tolerance`), passed in rather than guessed, so "differing"
 * here means exactly what it means in the diff's own cell counts.
 */
export function rankContributors(
  component: string,
  a: SeriesFrame,
  b: SeriesFrame,
  tolerance = 0,
): ContributorRanking {
  const left = keyed(a);
  const right = keyed(b);
  const mps = Array.from(new Set([...left.keys(), ...right.keys()])).sort();

  const rows: Contributor[] = [];
  let totalAbs = 0;
  let totalDelta = 0;
  for (const mp of mps) {
    const la = left.get(mp);
    const lb = right.get(mp);
    if (!la || !lb) continue; // one-sided modelpoints are the banner's business
    let delta = 0;
    let absDelta = 0;
    let cells = 0;
    let tFirst: number | null = null;
    const periods = Array.from(new Set([...la.keys(), ...lb.keys()])).sort((x, y) => x - y);
    for (const t of periods) {
      const va = la.get(t);
      const vb = lb.get(t);
      if (va === null || va === undefined || vb === null || vb === undefined) continue;
      const d = vb - va;
      if (!Number.isFinite(d)) continue;
      delta += d;
      absDelta += Math.abs(d);
      if (Math.abs(d) > tolerance) {
        cells += 1;
        if (tFirst === null) tFirst = t;
      }
    }
    rows.push({ mp, delta, absDelta, share: 0, cells, tFirst });
    totalAbs += absDelta;
    totalDelta += delta;
  }

  for (const row of rows) row.share = totalAbs === 0 ? 0 : row.absDelta / totalAbs;
  rows.sort((x, y) => (y.absDelta === x.absDelta ? (x.mp < y.mp ? -1 : 1) : y.absDelta - x.absDelta));

  return {
    component,
    contributors: rows,
    differing: rows.filter((r) => r.cells > 0).map((r) => r.mp),
    matching: rows.filter((r) => r.cells === 0).map((r) => r.mp),
    totalAbs,
    totalDelta,
    method: `Σ|b − a| over t per modelpoint, from both runs' series; |b − a| > ${tolerance} counts as a diverging cell`,
  };
}

/**
 * The absolute tolerance the diff applied to a component, read out of the
 * document's own tolerance profile rather than re-decided here.
 */
export function toleranceFor(tolerance: unknown, component: string): number {
  const t = tolerance as
    | {
        abs?: number;
        by_component?: Record<string, { abs?: number }>;
        overrides_applied?: { component: string; abs?: number }[];
      }
    | null
    | undefined;
  if (!t) return 0;
  const byComponent = t.by_component?.[component]?.abs;
  if (typeof byComponent === "number") return byComponent;
  const applied = t.overrides_applied?.find((o) => o.component === component);
  if (applied && typeof applied.abs === "number") return applied.abs;
  return typeof t.abs === "number" ? t.abs : 0;
}
