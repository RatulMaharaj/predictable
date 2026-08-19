import type { DataSource } from "../datasource";
import { RunDiffScreen } from "./RunDiffScreen";

export interface RunDiffRoute {
  a: string;
  b: string;
}

/**
 * Screen 2 claims `?view=diff&a=<run>&b=<run>` — the query string, per the
 * scaffold's convention; the fragment stays the server token.
 */
export function parseRunDiffRoute(search: string = location.search): RunDiffRoute | null {
  const params = new URLSearchParams(search);
  if (params.get("view") !== "diff") return null;
  const a = params.get("a");
  const b = params.get("b");
  if (!a || !b) return null;
  return { a, b };
}

export function isRunDiffUrl(search: string = location.search): boolean {
  return parseRunDiffRoute(search) !== null;
}

/** The URL `predictable diff run --show` opens, and the one the screen links to. */
export function runDiffUrl(route: RunDiffRoute, hash: string = location.hash): string {
  return `?view=diff&a=${encodeURIComponent(route.a)}&b=${encodeURIComponent(route.b)}${hash}`;
}

/**
 * Screen 2's root. "Show in graph" leaves the screen the same way Screen 3's
 * breadcrumbs do — by rewriting the query string, so the whole view is in the
 * URL and a reviewer can paste it into a ticket (§5.1).
 */
export function RunDiffRoot({
  source,
  route = parseRunDiffRoute()!,
}: {
  source: DataSource;
  route?: RunDiffRoute;
}) {
  return (
    <RunDiffScreen
      source={source}
      a={route.a}
      b={route.b}
      onShowInGraph={(component) => {
        const params = new URLSearchParams(location.search);
        params.delete("view");
        params.delete("a");
        params.delete("b");
        params.set("selected", component);
        location.search = params.toString();
      }}
    />
  );
}
