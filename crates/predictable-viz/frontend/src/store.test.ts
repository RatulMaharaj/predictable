import { describe, expect, it } from "vitest";
import { decodeView, encodeView, INITIAL_VIEW, type View } from "./store";

describe("URL as state (§5.1)", () => {
  const full: View = {
    selected: "model.qx",
    focus: true,
    impact: true,
    layers: true,
    lints: false,
    query: "NUM_POLS",
    filters: {
      modules: ["model", "schema"],
      units: ["money"],
      kinds: ["Output"],
      tags: ["ifrs17"],
      affects: "model.bel",
    },
    modelpoint: "POL00042",
    t: 3,
  };

  it("round-trips every field, transient panel state included", () => {
    expect(decodeView(encodeView(full))).toEqual(full);
  });

  it("round-trips the default view to an empty query string", () => {
    expect(encodeView(INITIAL_VIEW)).toBe("");
    expect(decodeView("")).toEqual(INITIAL_VIEW);
  });

  it("accepts a leading `?`", () => {
    expect(decodeView(`?${encodeView(full)}`)).toEqual(full);
  });

  it("defaults lints on and overlays off, so a bare URL is the plain graph", () => {
    const view = decodeView("sel=model.qx");
    expect(view).toMatchObject({ selected: "model.qx", lints: true, impact: false, focus: false });
  });

  it("keeps `t = 0` distinct from no `t` at all", () => {
    expect(decodeView(encodeView({ ...INITIAL_VIEW, t: 0 })).t).toBe(0);
    expect(decodeView("").t).toBeUndefined();
  });

  it("survives a hand-edited URL with empty facet lists", () => {
    expect(decodeView("module=&unit=,,&kind=Output").filters).toMatchObject({
      modules: [],
      units: [],
      kinds: ["Output"],
    });
  });
});
