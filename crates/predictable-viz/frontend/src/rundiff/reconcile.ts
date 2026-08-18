// Manifest reconciliation (05-viz.md §4.2, first bullet).
//
// "Half of all 'the numbers don't match' incidents are a different modelpoint
// file, and the tool should say so in the first second rather than the third
// hour." So this runs before a single number is rendered, and it reads only
// what the diff document already recorded — it never re-derives a digest.

import type { RunDiffDoc } from "../datasource";

export type ReconcileStatus = "same" | "different" | "unknown";

export interface ReconcileCheck {
  /** Stable id, used as the `data-check` attribute and the test handle. */
  key: string;
  label: string;
  status: ReconcileStatus;
  /** Side A's value, already shortened for display. */
  a: string;
  /** Side B's value. */
  b: string;
  /** Why this difference matters, when it does. */
  note?: string;
  /**
   * True when a difference here makes the *comparison itself* suspect rather
   * than being the finding. A different modelpoint set is not a model bug.
   */
  blocking: boolean;
}

/** `sha256:46e106…` → `46e106e`; the algorithm prefix is noise in a banner. */
export function shortDigest(digest: string | null | undefined): string {
  if (!digest) return "—";
  const colon = digest.indexOf(":");
  const body = colon === -1 ? digest : digest.slice(colon + 1);
  return body.length > 7 ? body.slice(0, 7) : body;
}

function cmp(a: string, b: string): ReconcileStatus {
  if (!a || !b) return "unknown";
  return a === b ? "same" : "different";
}

/**
 * The banner's rows, in the order §4.2 draws them: what was compared, then the
 * four digests, then the set reconciliation.
 *
 * A `.rpt` side A is a first-class case: its `model_digest` is empty, which
 * yields `unknown` rather than `different` — a Prophet run has no predictable
 * model digest and pretending otherwise would be a lie.
 */
export function reconcile(doc: RunDiffDoc): ReconcileCheck[] {
  const { a, b, summary } = doc;
  const checks: ReconcileCheck[] = [];

  checks.push({
    key: "system",
    label: "system",
    status: cmp(a.system, b.system),
    a: a.system,
    b: b.system,
    blocking: false,
    note:
      a.system !== b.system
        ? "cross-system comparison: side A was read into the results schema (04-verify §2)"
        : undefined,
  });

  checks.push({
    key: "model",
    label: "model digest",
    status: cmp(a.model_digest, b.model_digest),
    a: shortDigest(a.model_digest),
    b: shortDigest(b.model_digest),
    blocking: false,
    note:
      cmp(a.model_digest, b.model_digest) === "same"
        ? "same model on both sides — the movement is not a model change"
        : undefined,
  });

  checks.push({
    key: "manifest",
    label: "manifest",
    status: cmp(a.manifest, b.manifest),
    a: shortDigest(a.manifest),
    b: shortDigest(b.manifest),
    blocking: false,
  });

  checks.push({
    key: "engine",
    label: "engine",
    status: cmp(a.engine_version, b.engine_version),
    a: a.engine_version || "—",
    b: b.engine_version || "—",
    blocking: false,
    note:
      cmp(a.engine_version, b.engine_version) === "different"
        ? "different engine builds; determinism is only guaranteed within one version (03-engine §7)"
        : undefined,
  });

  const mp = summary.modelpoints;
  const mpDifferent = mp.only_a > 0 || mp.only_b > 0;
  checks.push({
    key: "modelpoints",
    label: "modelpoints",
    status: mpDifferent ? "different" : "same",
    a: String(mp.a),
    b: String(mp.b),
    blocking: mpDifferent,
    note: mpDifferent
      ? `${mp.common} compared · ${mp.only_a} only in A · ${mp.only_b} only in B — different modelpoint files explain more reconciliation failures than models do`
      : undefined,
  });

  const comp = summary.components;
  const compDifferent = comp.only_a > 0 || comp.only_b > 0;
  checks.push({
    key: "components",
    label: "components",
    status: compDifferent ? "different" : "same",
    a: String(comp.a),
    b: String(comp.b),
    blocking: false,
    note: compDifferent
      ? `${comp.common} compared · ${comp.only_a} only in A · ${comp.only_b} only in B`
      : undefined,
  });

  if (summary.emit_mismatch) {
    const em = summary.emit_mismatch;
    checks.push({
      key: "emit",
      label: "emit",
      status: "different",
      a: em.a,
      b: em.b,
      blocking: false,
      note: "different emit settings: the root divergence can only be localised to a component that was written down — re-run both sides with `emit = \"all\"`",
    });
  }

  if (summary.incomparable.length > 0) {
    checks.push({
      key: "incomparable",
      label: "incomparable",
      status: "different",
      a: summary.incomparable.join(", "),
      b: "—",
      blocking: true,
      note: "these could not be aligned at all; the verdict below covers the rest",
    });
  }

  return checks;
}

/** True when something above makes the numbers below untrustworthy. */
export function hasBlocking(checks: readonly ReconcileCheck[]): boolean {
  return checks.some((c) => c.blocking && c.status === "different");
}

/** The one-line headline: `12,481 modelpoints · 8 outputs · 6 match · 2 differ`. */
export function headline(doc: RunDiffDoc): string {
  const outputs = doc.summary.outputs;
  const within = outputs.filter((o) => o.within_tolerance).length;
  const differ = outputs.length - within;
  const n = (x: number) => x.toLocaleString("en-US");
  return (
    `${n(doc.summary.modelpoints.common)} modelpoints · ${n(outputs.length)} outputs · ` +
    `${n(within)} match within tolerance · ${n(differ)} differ`
  );
}
