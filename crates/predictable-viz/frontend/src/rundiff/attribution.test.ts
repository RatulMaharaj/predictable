import { describe, expect, it } from "vitest";
import type { RunDiffDoc } from "../datasource";
import { indexGraph } from "../graph/model";
import { attribution, orderFindings, shortName } from "./attribution";
import { WALKTHROUGH_DIFF, WALKTHROUGH_GRAPH } from "./__fixtures__";

const index = indexGraph(WALKTHROUGH_GRAPH);
const byId = (id: string) => WALKTHROUGH_DIFF.findings.find((f) => f.id === id)!;

describe("attribution", () => {
  it("turns the real root finding into a sentence about the edit", () => {
    const model = attribution(WALKTHROUGH_DIFF, byId("F001"), index);
    expect(model.class).toBe("root");
    expect(model.changed).toBe(true);
    expect(model.what).toEqual(["upstream"]);
    expect(model.sentence).toBe("an upstream change to premium_income explains this movement");
  });

  it("cross-references the planner's graph for the impact set", () => {
    const model = attribution(WALKTHROUGH_DIFF, byId("F001"), index);
    expect(model.impact).not.toBeNull();
    expect(model.impact!.components).toBeGreaterThan(0);
    // The impact set must cover every output the diff says diverged.
    for (const output of byId("F001").affects_outputs) {
      if (output === byId("F001").component) continue;
      expect(model.impact!.outputs).toContain(output);
    }
  });

  it("refuses to call an inherited finding a cause", () => {
    const model = attribution(WALKTHROUGH_DIFF, byId("F002"), index);
    expect(model.class).toBe("inherited");
    expect(model.sentence).toMatch(/^reserve moved because/);
    expect(model.sentence).toContain("it is not the cause");
  });

  it("is silent, not empty, when no model diff was supplied", () => {
    const doc: RunDiffDoc = {
      ...WALKTHROUGH_DIFF,
      summary: { ...WALKTHROUGH_DIFF.summary, model_attribution_available: false },
    };
    const model = attribution(doc, byId("F001"), index);
    expect(model.available).toBe(false);
    expect(model.changed).toBe(false);
    expect(model.sentence).toContain("no model diff was supplied");
  });

  it("omits the impact line rather than guessing when no graph is loaded", () => {
    expect(attribution(WALKTHROUGH_DIFF, byId("F001"), null).impact).toBeNull();
  });

  it("lists roots first, then by the share the diff itself computed", () => {
    const ordered = orderFindings(WALKTHROUGH_DIFF.findings);
    expect(ordered[0].id).toBe("F001");
    expect(ordered[0].class).toBe("root");
    const inherited = ordered.slice(1);
    const shares = inherited.map((f) => Math.abs(f.contribution?.share_of_total_delta ?? 0));
    expect([...shares].sort((a, b) => b - a)).toEqual(shares);
  });

  it("shortens ids for display without losing them", () => {
    expect(shortName("model.premium_income")).toBe("premium_income");
    expect(shortName("bel")).toBe("bel");
  });
});
