// URL-as-state for the drill-down (05-viz.md §5.1, §3.4).
//
// Every view is fully described by its URL, including transient panel state, so
// pasting a URL into a review comment reproduces the exact screen.
//
// The scaffold reserves the fragment for the server token (`main.tsx` reads
// `location.hash`), so screen state lives in the query string. The §3.4 path is
// carried in `view`:
//
//   ?view=portfolio&run=run-2026Q2
//   ?view=group&run=run-2026Q2&group=product%3DTERM%26cohort%3D2019
//   ?view=mp&run=run-2026Q2&mp=POL00042&t=3
//   ?view=mp&run=run-2026Q2&mp=POL00042&t=3&c=reserve      ← the explain() cell

export type Level = "portfolio" | "group" | "mp";

export interface DrilldownRoute {
  view: Level;
  run?: string;
  /** Level 2: the ordered key tuple of the declared `[[aggregation]]`. */
  group?: string;
  /** Level 3: the modelpoint key. */
  mp?: string;
  /** The selected period. */
  t?: number;
  /** The selected component — the entry point to `explain()`. */
  component?: string;
  /** Trace node paths the reader folded shut. */
  collapsed: string[];
  /** The "sort by magnitude" toggle. */
  sortByMagnitude: boolean;
  /** The heat overlay toggle. */
  heat: boolean;
}

export const DEFAULT_ROUTE: DrilldownRoute = {
  view: "portfolio",
  collapsed: [],
  sortByMagnitude: false,
  heat: false,
};

function isLevel(value: string | null): value is Level {
  return value === "portfolio" || value === "group" || value === "mp";
}

export function parseRoute(search: string): DrilldownRoute {
  const q = new URLSearchParams(search.startsWith("?") ? search.slice(1) : search);
  const view = q.get("view");
  const t = q.get("t");
  const parsed = t === null ? undefined : Number(t);
  return {
    view: isLevel(view) ? view : "portfolio",
    run: q.get("run") ?? undefined,
    group: q.get("group") ?? undefined,
    mp: q.get("mp") ?? undefined,
    t: parsed !== undefined && Number.isFinite(parsed) ? parsed : undefined,
    component: q.get("c") ?? undefined,
    collapsed: (q.get("collapsed") ?? "").split(",").filter(Boolean),
    sortByMagnitude: q.get("sort") === "magnitude",
    heat: q.get("heat") === "1",
  };
}

/** The inverse of {@link parseRoute}; keys are emitted in a stable order. */
export function formatRoute(route: DrilldownRoute): string {
  const q = new URLSearchParams();
  q.set("view", route.view);
  if (route.run) q.set("run", route.run);
  if (route.group) q.set("group", route.group);
  if (route.mp) q.set("mp", route.mp);
  if (route.t !== undefined) q.set("t", String(route.t));
  if (route.component) q.set("c", route.component);
  if (route.collapsed.length) q.set("collapsed", route.collapsed.join(","));
  if (route.sortByMagnitude) q.set("sort", "magnitude");
  if (route.heat) q.set("heat", "1");
  return `?${q}`;
}

export interface Crumb {
  label: string;
  route: DrilldownRoute;
}

/** The one breadcrumb §3.4 asks for: portfolio ▸ group ▸ modelpoint ▸ cell. */
export function breadcrumbs(route: DrilldownRoute): Crumb[] {
  const crumbs: Crumb[] = [
    { label: "portfolio", route: { ...DEFAULT_ROUTE, view: "portfolio", run: route.run } },
  ];
  if (route.group) {
    const base = { ...DEFAULT_ROUTE, view: "group" as const, run: route.run, group: route.group };
    for (const pair of route.group.split("&").filter(Boolean)) {
      crumbs.push({ label: pair, route: base });
    }
  }
  if (route.mp) {
    crumbs.push({
      label: route.mp,
      route: { ...route, view: "mp", component: undefined },
    });
  }
  if (route.component && route.t !== undefined) {
    crumbs.push({ label: `${route.component}@t=${route.t}`, route });
  }
  return crumbs;
}
