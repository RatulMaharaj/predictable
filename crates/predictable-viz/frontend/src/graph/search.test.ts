import { describe, expect, it } from "vitest";
import { TERM_ANNUAL, withProphet } from "../test/fixtures";
import { buildSearchIndex, fuzzyMatch, search } from "./search";

const plain = buildSearchIndex(TERM_ANNUAL);
const migrating = buildSearchIndex(withProphet());

describe("fuzzy matching", () => {
  it("matches a subsequence, case-insensitively", () => {
    expect(fuzzyMatch("npif", "num_pols_if")).not.toBeNull();
    expect(fuzzyMatch("NPIF", "num_pols_if")).not.toBeNull();
    expect(fuzzyMatch("zzz", "num_pols_if")).toBeNull();
  });

  it("reports the matched positions so the row can highlight them", () => {
    const m = fuzzyMatch("qx", "qx")!;
    expect(m.positions).toEqual([0, 1]);
  });

  it("prefers an exact match, then a prefix, then a scattered one", () => {
    const exact = fuzzyMatch("qx", "qx")!.score;
    const prefix = fuzzyMatch("qx", "qx_loaded")!.score;
    const scattered = fuzzyMatch("qx", "quarterly_matrix")!.score;
    expect(exact).toBeGreaterThan(prefix);
    expect(prefix).toBeGreaterThan(scattered);
  });

  it("rewards a hit at a word boundary, `_` and `.` included", () => {
    expect(fuzzyMatch("pols", "num_pols_if")!.score).toBeGreaterThan(
      fuzzyMatch("ols", "num_pols_if")!.score,
    );
  });
});

describe("the palette index", () => {
  it("indexes names, modules, tags, docs and exprs", () => {
    const fields = new Set(plain.map((e) => e.field));
    expect(fields).toContain("name");
    expect(fields).toContain("module");
    expect(fields).toContain("expr");
  });

  it("finds a component by its own name", () => {
    const hits = search(plain, "num_pols");
    expect(hits[0].id).toBe("model.num_pols_if");
    expect(hits[0].field).toBe("name");
  });

  it("returns one row per component, not one per matching field", () => {
    const hits = search(plain, "qx");
    expect(new Set(hits.map((h) => h.id)).size).toBe(hits.length);
  });

  it("lists components in plan order when the query is empty", () => {
    const hits = search(plain, "");
    expect(hits[0].id).toBe(TERM_ANNUAL.nodes[0].id);
  });

  it("says so, rather than throwing, when nothing matches", () => {
    expect(search(plain, "zzzzqqqq")).toEqual([]);
  });
});

describe("Prophet variable names — the migration path (§2.3)", () => {
  it("typing NUM_POLS_IF lands on the component that claims it", () => {
    // Here the predictable name is the Prophet name lower-cased, so the `name`
    // field legitimately wins the row; what matters is that the Prophet key was
    // indexed and the actuary lands on the right component either way.
    expect(search(migrating, "NUM_POLS_IF")[0].id).toBe("model.num_pols_if");
    expect(
      migrating.some((e) => e.field === "prophet" && e.text === "TERM_UK.NUM_POLS_IF"),
    ).toBe(true);
  });

  it("finds a component whose predictable name shares nothing with the Prophet one", () => {
    // `BEL_TOT` → `reserve`: no substring in common, which is exactly the case
    // an actuary cannot solve with ⌘F over the source.
    const hits = search(migrating, "BEL_TOT");
    expect(hits[0].id).toBe("model.reserve");
    expect(hits[0].field).toBe("prophet");
  });

  it("finds every component of a Prophet library by its library name", () => {
    const hits = search(migrating, "TERM_UK");
    expect(hits.length).toBe(3);
    expect(hits.every((h) => h.field === "prophet")).toBe(true);
  });

  it("still ranks an exact predictable name above a Prophet near-match", () => {
    const hits = search(migrating, "reserve");
    expect(hits[0].id).toBe("model.reserve");
    expect(hits[0].field).toBe("name");
  });

  it("indexes nothing extra for a model with no provenance", () => {
    expect(plain.some((e) => e.field === "prophet")).toBe(false);
    expect(search(plain, "NUM_POLS_IF")[0]?.field).not.toBe("prophet");
  });
});
