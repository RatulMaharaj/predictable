import type { DataSource } from "../datasource";
import { DrilldownScreen, useDrilldownRoute } from "./DrilldownScreen";
import { parseRoute } from "./route";

/** True when the current URL addresses screen 3 (`?view=mp&mp=…`). */
export function isDrilldownUrl(search: string = location.search): boolean {
  const route = parseRoute(search);
  return route.view === "mp" && route.mp !== undefined;
}

/**
 * Screen 3's root: it owns the route (the query string, per the scaffold's
 * convention — the fragment stays the server token) and nothing else.
 */
export function DrilldownRoot({ source }: { source: DataSource }) {
  const [route, setRoute] = useDrilldownRoute();
  return <DrilldownScreen source={source} route={route} onRouteChange={setRoute} />;
}
