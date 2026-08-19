// The attribution panel's data model (05-viz.md §4.2, second bullet).
//
// "Attribution, not just difference." The left panel is the *model* diff's
// answer — `finding.explained_by_model_change` (04-verify §5.4) — cross-
// referenced with the impact set the planner's graph already implies. Nothing
// here re-classifies a finding or invents a cause: `root` vs `inherited` is the
// diff's call, and this module only phrases it.

import type { Finding, RunDiffDoc } from "../datasource";
import { impact as impactOf, type GraphIndex } from "../graph/model";

export interface AttributionImpact {
  /** How many other components the changed one reaches. */
  components: number;
  /** Reported outputs inside that reach, named — §4.2 shows the names. */
  outputs: string[];
}

export interface Attribution {
  finding: string;
  component: string;
  class: string;
  /** `ir_graph` / `partial_graph` / `no_graph_earliest_t`: how `class` was decided. */
  classBasis: string;
  /**
   * False when the diff had no model diff to consult. §5.4 is explicit that
   * this makes `explained_by_model_change` *silent*, not empty — so the panel
   * must say "no model diff was supplied", never "no model change".
   */
  available: boolean;
  changed: boolean;
  /** `formula` / `init` / `timing` / `unit` / `upstream` / … */
  what: string[];
  /** For an `inherited` finding, the component the diff blames instead. */
  sourceComponent: string | null;
  affectsOutputs: string[];
  /** Null when no `GraphDoc` is loaded; the panel then omits the impact line. */
  impact: AttributionImpact | null;
  /** The sentence §4.2 wants: a finding, not a difference. */
  sentence: string;
}

function phraseWhat(what: readonly string[]): string {
  if (what.length === 0) return "a model change";
  if (what.length === 1) {
    const article = /^[aeiou]/i.test(what[0]) ? "an" : "a";
    return `${article} ${what[0]} change`;
  }
  return `${what.slice(0, -1).join(", ")} and ${what[what.length - 1]} changes`;
}

/**
 * Build the panel model for one finding.
 *
 * `index` is optional: with no graph the panel still renders everything the
 * diff document knows, minus the impact-set line.
 */
export function attribution(
  doc: RunDiffDoc,
  finding: Finding,
  index?: GraphIndex | null,
): Attribution {
  const available = doc.summary.model_attribution_available;
  const explained = finding.explained_by_model_change;
  const changed = available && (explained?.changed ?? false);
  const what = explained?.what ?? [];

  let impact: AttributionImpact | null = null;
  if (index && index.byId.has(finding.component)) {
    const got = impactOf(index, finding.component);
    const outputs = Array.from(got.ids)
      .filter((id) => index.byId.get(id)?.kind === "Output")
      .sort();
    impact = { components: got.components, outputs };
  }

  const name = shortName(finding.component);
  let sentence: string;
  if (!available) {
    sentence = `no model diff was supplied, so nothing can be said about *why* ${name} moved — re-run with \`--model-a\`/\`--model-b\``;
  } else if (finding.class === "root" && changed) {
    sentence = `${phraseWhat(what)} to ${name} explains this movement`;
  } else if (finding.class === "root") {
    sentence = `${name} is the earliest component that moved, and the model diff found no edit that explains it — look at inputs, tables or assumptions`;
  } else if (finding.class === "inherited") {
    const from = finding.source_component ? shortName(finding.source_component) : "an upstream component";
    sentence = `${name} moved because ${from} moved; it is not the cause`;
  } else if (finding.class === "structural") {
    sentence = `${name} exists on only one side, so its cells were never compared`;
  } else {
    sentence = `${name} differs only within the tolerance that was applied`;
  }

  return {
    finding: finding.id,
    component: finding.component,
    class: finding.class,
    classBasis: finding.class_basis,
    available,
    changed,
    what,
    sourceComponent: finding.source_component,
    affectsOutputs: finding.affects_outputs,
    impact,
    sentence,
  };
}

/** `model.premium_income` → `premium_income`; the module is in the header. */
export function shortName(id: string): string {
  const dot = id.lastIndexOf(".");
  return dot === -1 ? id : id.slice(dot + 1);
}

/**
 * Findings in the order the screen lists them: roots first (that is the whole
 * point of the classification), then by share of the delta they claim, then by
 * id so the list is stable across reloads.
 */
export function orderFindings(findings: readonly Finding[]): Finding[] {
  const rank = (f: Finding) =>
    f.class === "root" ? 0 : f.class === "inherited" ? 1 : f.class === "structural" ? 2 : 3;
  return findings.slice().sort((x, y) => {
    if (rank(x) !== rank(y)) return rank(x) - rank(y);
    const sx = Math.abs(x.contribution?.share_of_total_delta ?? 0);
    const sy = Math.abs(y.contribution?.share_of_total_delta ?? 0);
    if (sx !== sy) return sy - sx;
    return x.id < y.id ? -1 : 1;
  });
}
