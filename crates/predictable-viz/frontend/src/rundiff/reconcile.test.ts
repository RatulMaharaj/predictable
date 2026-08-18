import { describe, expect, it } from "vitest";
import type { RunDiffDoc } from "../datasource";
import { hasBlocking, headline, reconcile, shortDigest } from "./reconcile";
import { WALKTHROUGH_DIFF } from "./__fixtures__";

const check = (doc: RunDiffDoc, key: string) => reconcile(doc).find((c) => c.key === key)!;

describe("manifest reconciliation", () => {
  it("reads the walkthrough's real diff: same modelpoints, different models", () => {
    const doc = WALKTHROUGH_DIFF;
    expect(check(doc, "modelpoints").status).toBe("same");
    expect(check(doc, "modelpoints").a).toBe("25");
    expect(check(doc, "components").status).toBe("same");
    expect(check(doc, "model").status).toBe("different");
    expect(check(doc, "manifest").status).toBe("different");
    expect(check(doc, "engine").status).toBe("same");
    expect(hasBlocking(reconcile(doc))).toBe(false);
  });

  it("calls a different modelpoint file blocking, before any number is read", () => {
    const doc: RunDiffDoc = {
      ...WALKTHROUGH_DIFF,
      summary: {
        ...WALKTHROUGH_DIFF.summary,
        modelpoints: { a: 25, b: 24, common: 24, only_a: 1, only_b: 0 },
      },
    };
    const mp = check(doc, "modelpoints");
    expect(mp.status).toBe("different");
    expect(mp.blocking).toBe(true);
    expect(mp.note).toContain("1 only in A");
    expect(hasBlocking(reconcile(doc))).toBe(true);
  });

  it("does not claim a Prophet side has a matching model digest", () => {
    const doc: RunDiffDoc = {
      ...WALKTHROUGH_DIFF,
      a: { ...WALKTHROUGH_DIFF.a, system: "prophet", model_digest: "" },
    };
    expect(check(doc, "model").status).toBe("unknown");
    expect(check(doc, "system").status).toBe("different");
    expect(check(doc, "system").note).toContain("results schema");
  });

  it("surfaces an emit mismatch as its own row", () => {
    const doc: RunDiffDoc = {
      ...WALKTHROUGH_DIFF,
      summary: {
        ...WALKTHROUGH_DIFF.summary,
        emit_mismatch: {
          a: "outputs",
          b: "all",
          a_component_set_digest: "sha256:aa",
          b_component_set_digest: "sha256:bb",
        },
      },
    };
    const emit = check(doc, "emit");
    expect(emit.status).toBe("different");
    expect(emit.note).toContain('emit = "all"');
  });

  it("headlines the outputs the way §4.2 phrases it", () => {
    expect(headline(WALKTHROUGH_DIFF)).toBe(
      "25 modelpoints · 13 outputs · 7 match within tolerance · 6 differ",
    );
  });

  it("shortens digests and survives a missing one", () => {
    expect(shortDigest("sha256:46e106e5ab")).toBe("46e106e");
    expect(shortDigest(undefined)).toBe("—");
  });
});
