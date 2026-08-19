import { describe, expect, it } from "vitest";
import { isRunDiffUrl, parseRunDiffRoute, runDiffUrl } from "./mount";

describe("screen 2's route", () => {
  it("claims `?view=diff` only when both runs are named", () => {
    expect(isRunDiffUrl("?view=diff&a=r1&b=r2")).toBe(true);
    expect(isRunDiffUrl("?view=diff&a=r1")).toBe(false);
    expect(isRunDiffUrl("?view=mp&mp=TA00001")).toBe(false);
    expect(isRunDiffUrl("")).toBe(false);
  });

  it("round-trips run ids that contain colons", () => {
    const route = { a: "2026-08-17T08:08:59Z-6b512e30", b: "2026-08-17T08:08:59Z-1e9bd949" };
    expect(parseRunDiffRoute(runDiffUrl(route, ""))).toEqual(route);
  });

  it("keeps the server token in the fragment, not the query", () => {
    expect(runDiffUrl({ a: "r1", b: "r2" }, "#tok")).toBe("?view=diff&a=r1&b=r2#tok");
  });
});
