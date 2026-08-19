import { describe, expect, it } from "vitest";
import { frameOf, rankContributors, toleranceFor } from "./contributors";
import { WALKTHROUGH_DIFF, WalkthroughSource } from "./__fixtures__";

const source = new WalkthroughSource();
const doc = WALKTHROUGH_DIFF;
const COMPONENT = "model.premium_income";

async function ranking(tolerance = 0.005) {
  const [a, b] = await Promise.all([
    source.series({ run: doc.a.run, components: [COMPONENT] }),
    source.series({ run: doc.b.run, components: [COMPONENT] }),
  ]);
  return rankContributors(COMPONENT, frameOf(a, COMPONENT), frameOf(b, COMPONENT), tolerance);
}

describe("contributor ranking over the walkthrough's real runs", () => {
  it("agrees with the diff's own modelpoint count", async () => {
    const r = await ranking();
    const finding = doc.findings.find((f) => f.component === COMPONENT)!;
    expect(r.differing.length).toBe(finding.n_modelpoints);
    expect(r.matching).toEqual([]);
  });

  it("puts the diff's `worst` modelpoint at the top of the bars", async () => {
    const r = await ranking();
    const finding = doc.findings.find((f) => f.component === COMPONENT)!;
    expect(r.contributors[0].mp).toBe(finding.worst.mp_key);
    expect(r.contributors[0].tFirst).toBe(finding.t_first);
  });

  it("reproduces the exemplar cell exactly", async () => {
    const finding = doc.findings.find((f) => f.component === COMPONENT)!;
    const [a, b] = await Promise.all([
      source.series({ run: doc.a.run, components: [COMPONENT] }),
      source.series({ run: doc.b.run, components: [COMPONENT] }),
    ]);
    const fa = frameOf(a, COMPONENT);
    const fb = frameOf(b, COMPONENT);
    const i = fa.mp.findIndex((mp, k) => mp === finding.exemplar.mp_key && fa.t[k] === finding.exemplar.t);
    expect(fa.values[i]).toBeCloseTo(finding.exemplar.a as number, 6);
    expect(fb.values[i]).toBeCloseTo(finding.exemplar.b as number, 6);
  });

  it("shares sum to one and are ordered by bar length", async () => {
    const r = await ranking();
    const total = r.contributors.reduce((s, c) => s + c.share, 0);
    expect(total).toBeCloseTo(1, 9);
    const bars = r.contributors.map((c) => c.absDelta);
    expect([...bars].sort((x, y) => y - x)).toEqual(bars);
  });

  it("states how the numbers were derived", async () => {
    expect((await ranking()).method).toContain("Σ|b − a| over t per modelpoint");
  });

  it("counts nothing as differing when the tolerance swallows the movement", async () => {
    const r = await ranking(1e9);
    expect(r.differing).toEqual([]);
    expect(r.matching.length).toBe(25);
  });

  it("reads the tolerance the diff applied, not a guess", () => {
    expect(toleranceFor(doc.tolerance, "model.premium_income")).toBe(0.005);
    expect(toleranceFor(doc.tolerance, "model.deaths")).toBe(1e-6);
    expect(toleranceFor(null, "anything")).toBe(0);
  });

  it("rejects a batch that is missing the component's column", async () => {
    const table = await source.series({ run: doc.a.run, components: [COMPONENT] });
    expect(() => frameOf(table, "model.nope")).toThrow(/no column/);
  });
});
