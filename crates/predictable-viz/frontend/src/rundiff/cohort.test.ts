import { describe, expect, it } from "vitest";
import { cohortSentence, describe as describePredicate, detectCohort, type MpFields } from "./cohort";
import { frameOf } from "./contributors";
import { WALKTHROUGH_DIFF, WalkthroughSource } from "./__fixtures__";

const source = new WalkthroughSource();
const FIELDS = ["schema.entry_age", "schema.sum_assured", "schema.policy_term", "schema.expense_band", "schema.smoker"];

/** The declared modelpoint fields, exactly as the screen fetches them. */
async function fields(): Promise<MpFields> {
  const table = await source.series({
    run: WALKTHROUGH_DIFF.b.run,
    components: FIELDS,
    tFrom: 0,
    tTo: 0,
  });
  const out: MpFields = new Map();
  for (const id of FIELDS) {
    const frame = frameOf(table, id);
    const values = new Map<string, number>();
    frame.mp.forEach((mp, i) => {
      const v = frame.values[i];
      if (v !== null && Number.isFinite(v)) values.set(mp, v);
    });
    out.set(id, values);
  }
  return out;
}

describe("cohort detection", () => {
  it("says there is no cohort when the change bites every modelpoint", async () => {
    const f = await fields();
    const all = Array.from(f.get("schema.entry_age")!.keys());
    const result = detectCohort(f, all, []);
    expect(result.kind).toBe("all");
    expect(cohortSentence(result)).toBe(
      "all 25 modelpoints differ — there is no cohort, the change bites everywhere",
    );
  });

  it("finds the depth-2 split that actually separates the differing set", async () => {
    const f = await fields();
    const ages = f.get("schema.entry_age")!;
    const smoker = f.get("schema.smoker")!;
    const differing: string[] = [];
    const matching: string[] = [];
    for (const [mp, age] of ages) {
      (smoker.get(mp) === 1 && age >= 45 ? differing : matching).push(mp);
    }
    expect(differing.length).toBeGreaterThan(0);
    expect(matching.length).toBeGreaterThan(0);

    const result = detectCohort(f, differing, matching);
    expect(result.kind).toBe("cohort");
    if (result.kind !== "cohort") return;
    expect(result.covers).toBe(differing.length);
    expect(result.falsePositives).toBe(0);
    expect(result.predicates).toHaveLength(2);
    expect(new Set(result.predicates.map((p) => p.field)).size).toBe(2);
    // Whatever fields it picked, the claim it makes must be true of the data.
    const holds = (mp: string) =>
      result.predicates.every((p) => {
        const v = f.get(p.field)!.get(mp)!;
        return p.op === ">=" ? v >= p.value : p.op === "<=" ? v <= p.value : v === p.value;
      });
    expect(differing.every(holds)).toBe(true);
    expect(matching.some(holds)).toBe(false);
    expect(cohortSentence(result)).toContain(`all ${differing.length} share:`);
    expect(result.derived).toBe(true);
  });

  it("finds a depth-1 split and does not pad it to two", async () => {
    const f = await fields();
    const band = f.get("schema.expense_band")!;
    const differing: string[] = [];
    const matching: string[] = [];
    for (const [mp, v] of band) (v === 1 ? differing : matching).push(mp);
    const result = detectCohort(f, differing, matching);
    expect(result.kind).toBe("cohort");
    if (result.kind !== "cohort") return;
    expect(result.predicates).toHaveLength(1);
    expect(describePredicate(result.predicates[0])).toBe("expense_band = 1");
    expect(result.precision).toBe(1);
    expect(result.recall).toBe(1);
  });

  it("stays silent rather than inventing a predicate for a random split", async () => {
    const f = await fields();
    const mps = Array.from(f.get("schema.entry_age")!.keys()).sort();
    // Alternating membership is uncorrelated with every declared field.
    const differing = mps.filter((_, i) => i % 2 === 0);
    const matching = mps.filter((_, i) => i % 2 === 1);
    const result = detectCohort(f, differing, matching);
    expect(result.kind).toBe("none");
    expect(cohortSentence(result)).toContain("no field predicate separates");
  });

  it("says so when there are no fields to split on", () => {
    const result = detectCohort(new Map(), ["A"], ["B"]);
    expect(result.kind).toBe("none");
    if (result.kind !== "none") return;
    expect(result.reason).toContain("no declared modelpoint fields");
  });
});
