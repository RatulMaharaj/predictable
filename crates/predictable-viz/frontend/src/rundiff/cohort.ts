// Cohort detection (05-viz.md §4.2, third bullet).
//
// "Given the set of differing modelpoints, report the modelpoint-field
// predicates that best separate them from the matching ones (a depth-2
// decision-tree split over declared modelpoint fields, presented as a
// hypothesis and labelled as such)."
//
// Two disciplines the spec is explicit about and this file keeps:
//
//  1. It is a **chart-only derived quantity** under §0. Every result carries
//     `derived: true` and the method string, and the UI labels it a hypothesis.
//  2. It is allowed to fail. "All 25 modelpoints differ" and "no field
//     separates them" are answers, not empty states — a fabricated predicate
//     that covers 60 % of the differing set is worse than silence.

export type Comparison = ">=" | "<=" | "==";

export interface Predicate {
  /** Declared modelpoint field (an `InputModelpoint` component id). */
  field: string;
  op: Comparison;
  value: number;
}

export interface Cohort {
  kind: "cohort";
  /** Conjunction, depth 1 or 2. */
  predicates: Predicate[];
  /** Differing modelpoints the conjunction covers. */
  covers: number;
  /** Differing modelpoints in total. */
  ofDiffering: number;
  /** Matching modelpoints it wrongly sweeps in. */
  falsePositives: number;
  /** covers / (covers + falsePositives). */
  precision: number;
  /** covers / ofDiffering. */
  recall: number;
  derived: true;
  method: string;
}

export interface AllDiffer {
  kind: "all";
  n: number;
  derived: true;
  method: string;
}

export interface NoCohort {
  kind: "none";
  reason: string;
  derived: true;
  method: string;
}

export type CohortResult = Cohort | AllDiffer | NoCohort;

/** Declared modelpoint fields, `field id → (mp key → numeric value)`. */
export type MpFields = Map<string, Map<string, number>>;

const METHOD =
  "depth-2 decision-tree split over declared modelpoint fields, maximising F1 against the differing set";

function test(p: Predicate, v: number | undefined): boolean {
  if (v === undefined || !Number.isFinite(v)) return false;
  if (p.op === ">=") return v >= p.value;
  if (p.op === "<=") return v <= p.value;
  return v === p.value;
}

function candidates(field: string, values: Map<string, number>): Predicate[] {
  const unique = Array.from(new Set(values.values()))
    .filter((v) => Number.isFinite(v))
    .sort((a, b) => a - b);
  if (unique.length < 2) return [];
  const out: Predicate[] = [];
  // Thresholds sit *on* observed values, so the predicate the actuary reads
  // ("entry_age >= 55") is a value that exists in the modelpoint file.
  const capped = unique.length > 64 ? sample(unique, 64) : unique;
  for (const v of capped) {
    out.push({ field, op: ">=", value: v });
    out.push({ field, op: "<=", value: v });
  }
  if (unique.length <= 8) for (const v of unique) out.push({ field, op: "==", value: v });
  return out;
}

function sample(values: number[], n: number): number[] {
  const step = values.length / n;
  const out: number[] = [];
  for (let i = 0; i < n; i++) out.push(values[Math.floor(i * step)]);
  return Array.from(new Set(out));
}

/**
 * Ties are common — several thresholds separate the same 25 modelpoints
 * equally well — so the comparator is total and deterministic: better F1, then
 * the simpler split, then equalities (which read as a cohort) over
 * inequalities, then lexicographic order.
 */
function better(candidate: Scored, incumbent: Scored | null): boolean {
  if (!incumbent) return true;
  if (candidate.f1 !== incumbent.f1) return candidate.f1 > incumbent.f1;
  if (candidate.predicates.length !== incumbent.predicates.length) {
    return candidate.predicates.length < incumbent.predicates.length;
  }
  const eq = (s: Scored) => s.predicates.filter((p) => p.op === "==").length;
  if (eq(candidate) !== eq(incumbent)) return eq(candidate) > eq(incumbent);
  const key = (s: Scored) => s.predicates.map((p) => `${p.field}${p.op}${p.value}`).join("&");
  return key(candidate) < key(incumbent);
}

interface Scored {
  predicates: Predicate[];
  covers: number;
  falsePositives: number;
  f1: number;
}

function score(predicates: Predicate[], fields: MpFields, differing: string[], matching: string[]): Scored {
  const holds = (mp: string) =>
    predicates.every((p) => test(p, fields.get(p.field)?.get(mp)));
  let covers = 0;
  for (const mp of differing) if (holds(mp)) covers += 1;
  let falsePositives = 0;
  for (const mp of matching) if (holds(mp)) falsePositives += 1;
  const precision = covers + falsePositives === 0 ? 0 : covers / (covers + falsePositives);
  const recall = differing.length === 0 ? 0 : covers / differing.length;
  const f1 = precision + recall === 0 ? 0 : (2 * precision * recall) / (precision + recall);
  return { predicates, covers, falsePositives, f1 };
}

/**
 * Find the cohort, or say why there is none.
 *
 * `minF1` is the floor below which nothing is reported: a split that neither
 * covers the differing set nor excludes the matching one is noise, and §4.2's
 * sentence ("all 412 share…") must stay true when it is shown.
 */
export function detectCohort(
  fields: MpFields,
  differing: readonly string[],
  matching: readonly string[],
  minF1 = 0.8,
): CohortResult {
  const diff = Array.from(differing);
  const match = Array.from(matching);
  if (diff.length === 0) {
    return { kind: "none", reason: "nothing differs", derived: true, method: METHOD };
  }
  if (match.length === 0) {
    return { kind: "all", n: diff.length, derived: true, method: METHOD };
  }
  if (fields.size === 0) {
    return {
      kind: "none",
      reason: "no declared modelpoint fields were available to split on",
      derived: true,
      method: METHOD,
    };
  }

  const pool: Predicate[] = [];
  for (const [field, values] of fields) pool.push(...candidates(field, values));
  if (pool.length === 0) {
    return {
      kind: "none",
      reason: "every declared modelpoint field is constant across the run",
      derived: true,
      method: METHOD,
    };
  }

  let best: Scored | null = null;
  const singles: Scored[] = [];
  for (const p of pool) {
    const s = score([p], fields, diff, match);
    singles.push(s);
    if (better(s, best)) best = s;
  }

  // Depth 2: refine the strongest depth-1 splits with a second predicate from
  // a *different* field, which is what makes the sentence readable
  // ("smoker = true, entry_age >= 55") rather than an interval in disguise.
  singles.sort((a, b) => b.f1 - a.f1);
  for (const seed of singles.slice(0, 8)) {
    for (const p of pool) {
      if (p.field === seed.predicates[0].field) continue;
      const s = score([seed.predicates[0], p], fields, diff, match);
      if (better(s, best)) best = s;
    }
  }

  if (!best || best.f1 < minF1) {
    return {
      kind: "none",
      reason: `no field predicate separates the ${diff.length} differing modelpoints from the ${match.length} matching ones`,
      derived: true,
      method: METHOD,
    };
  }

  return {
    kind: "cohort",
    predicates: best.predicates,
    covers: best.covers,
    ofDiffering: diff.length,
    falsePositives: best.falsePositives,
    precision:
      best.covers + best.falsePositives === 0
        ? 0
        : best.covers / (best.covers + best.falsePositives),
    recall: best.covers / diff.length,
    derived: true,
    method: METHOD,
  };
}

/** `schema.entry_age >= 55` → `entry_age ≥ 55`. */
export function describe(p: Predicate): string {
  const dot = p.field.lastIndexOf(".");
  const name = dot === -1 ? p.field : p.field.slice(dot + 1);
  const op = p.op === ">=" ? "≥" : p.op === "<=" ? "≤" : "=";
  const value = Number.isInteger(p.value) ? String(p.value) : String(p.value);
  return `${name} ${op} ${value}`;
}

/** The one sentence §4.2 puts under the bars. */
export function cohortSentence(result: CohortResult): string {
  if (result.kind === "all") {
    return `all ${result.n} modelpoints differ — there is no cohort, the change bites everywhere`;
  }
  if (result.kind === "none") return result.reason;
  const clause = result.predicates.map(describe).join(", ");
  const exact = result.covers === result.ofDiffering && result.falsePositives === 0;
  const head = exact
    ? `all ${result.ofDiffering} share: ${clause}`
    : `${result.covers} of ${result.ofDiffering} share: ${clause}`;
  return result.falsePositives === 0
    ? head
    : `${head} (and ${result.falsePositives} matching modelpoints do too)`;
}
