import { describe, expect, it } from "vitest";
import { breadcrumbs, DEFAULT_ROUTE, formatRoute, parseRoute } from "./route";

describe("URL as state (§5.1)", () => {
  it("round-trips a full drill-down URL", () => {
    const url =
      "?view=mp&run=run-2026Q2&group=product%3DTERM%26cohort%3D2019&mp=POL00042&t=3&c=reserve&collapsed=root.children%5B0%5D&sort=magnitude&heat=1";
    const route = parseRoute(url);
    expect(route).toEqual({
      view: "mp",
      run: "run-2026Q2",
      group: "product=TERM&cohort=2019",
      mp: "POL00042",
      t: 3,
      component: "reserve",
      collapsed: ["root.children[0]"],
      sortByMagnitude: true,
      heat: true,
    });
    expect(parseRoute(formatRoute(route))).toEqual(route);
  });

  it("keeps t = 0 rather than losing it to a falsy check", () => {
    expect(parseRoute("?view=mp&mp=A&t=0").t).toBe(0);
    expect(formatRoute({ ...DEFAULT_ROUTE, view: "mp", mp: "A", t: 0 })).toContain("t=0");
  });

  it("omits everything unset, so a shared URL carries no noise", () => {
    expect(formatRoute(DEFAULT_ROUTE)).toBe("?view=portfolio");
  });

  it("falls back to the portfolio level for junk rather than throwing", () => {
    expect(parseRoute("?view=nonsense&t=abc")).toEqual(DEFAULT_ROUTE);
    expect(parseRoute("")).toEqual(DEFAULT_ROUTE);
  });

  it("builds the portfolio ▸ group ▸ modelpoint ▸ cell breadcrumb", () => {
    const crumbs = breadcrumbs(parseRoute("?view=mp&group=product%3DTERM%26cohort%3D2019&mp=POL00042&t=3&c=reserve"));
    expect(crumbs.map((c) => c.label)).toEqual([
      "portfolio",
      "product=TERM",
      "cohort=2019",
      "POL00042",
      "reserve@t=3",
    ]);
  });

  it("clears the cell selection when you click back to the modelpoint", () => {
    const crumbs = breadcrumbs(parseRoute("?view=mp&mp=POL00042&t=3&c=reserve"));
    const mpCrumb = crumbs.find((c) => c.label === "POL00042")!;
    expect(mpCrumb.route.component).toBeUndefined();
    expect(mpCrumb.route.mp).toBe("POL00042");
  });
});
