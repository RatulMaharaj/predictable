import { useCallback, useEffect, useMemo, useState } from "react";
import type { DataSource, GraphDoc, GraphNode } from "../datasource";
import { ModelpointGrid } from "./ModelpointGrid";
import { TracePanel } from "./TracePanel";
import { Waterfall } from "./WaterfallChart";
import { matrixFromArrow, type Selection, type SeriesMatrix } from "./grid";
import { breadcrumbs, formatRoute, parseRoute, type DrilldownRoute } from "./route";
import type { Flow } from "./waterfall";
import type { Trace } from "./trace";
import "./drilldown.css";

export interface DrilldownScreenProps {
  source: DataSource;
  route: DrilldownRoute;
  onRouteChange: (route: DrilldownRoute) => void;
  /** Periods to fetch; the grid virtualises, the request does not need all of T. */
  tFrom?: number;
  tTo?: number;
}

/**
 * Screen 3 — modelpoint drill-down (05-viz.md §4.3).
 *
 * Grid, waterfall and trace are the same data at three zoom levels: selecting a
 * grid cell moves the waterfall's `t` and opens `explain()` for that component;
 * clicking a bar moves the trace; hovering a trace node highlights the bar.
 * All of it is in the URL.
 */
export function DrilldownScreen({
  source,
  route,
  onRouteChange,
  tFrom = 0,
  tTo = 40,
}: DrilldownScreenProps) {
  const [graph, setGraph] = useState<GraphDoc | null>(null);
  const [matrix, setMatrix] = useState<SeriesMatrix | null>(null);
  const [trace, setTrace] = useState<Trace | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [traceError, setTraceError] = useState<string | null>(null);
  const [showTable, setShowTable] = useState(false);
  const [highlight, setHighlight] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    source
      .graph()
      .then((g) => live && setGraph(g))
      .catch((e: Error) => live && setError(e.message));
    return () => {
      live = false;
    };
  }, [source]);

  // Rows are the model's own Series components in declaration order (IR §4.1
  // rule 2) — the grid never invents an order the model did not state.
  const seriesNodes = useMemo<GraphNode[]>(
    () =>
      (graph?.nodes ?? [])
        .filter((n) => n.shape === "Series")
        .slice()
        .sort((a, b) => a.declaration_index - b.declaration_index),
    [graph],
  );

  useEffect(() => {
    if (!seriesNodes.length || !route.mp) return;
    let live = true;
    const components = seriesNodes.map((n) => n.id);
    source
      .series({ run: route.run, components, modelpoints: [route.mp], tFrom, tTo })
      .then((table) => live && setMatrix(matrixFromArrow(table, components)))
      .catch((e: Error) => live && setError(e.message));
    return () => {
      live = false;
    };
  }, [source, seriesNodes, route.mp, route.run, tFrom, tTo]);

  useEffect(() => {
    if (!route.mp || !route.component || route.t === undefined) {
      setTrace(null);
      return;
    }
    let live = true;
    setTraceError(null);
    source
      .explain({ run: route.run, component: route.component, modelpoint: route.mp, t: route.t })
      .then((doc) => live && setTrace(doc as Trace))
      .catch((e: Error) => {
        if (!live) return;
        setTrace(null);
        setTraceError(e.message);
      });
    return () => {
      live = false;
    };
  }, [source, route.run, route.mp, route.component, route.t]);

  const patch = useCallback(
    (next: Partial<DrilldownRoute>) => onRouteChange({ ...route, ...next }),
    [onRouteChange, route],
  );

  const selection = useMemo<Selection>(() => {
    const row = Math.max(
      0,
      matrix && route.component ? matrix.components.indexOf(route.component) : 0,
    );
    const col = Math.max(0, matrix && route.t !== undefined ? matrix.periods.indexOf(route.t) : 0);
    const cell = { row, col };
    return { anchor: cell, focus: cell };
  }, [matrix, route.component, route.t]);

  const onSelectionChange = useCallback(
    (next: Selection) => {
      if (!matrix) return;
      patch({
        component: matrix.components[next.focus.row],
        t: matrix.periods[next.focus.col],
      });
    },
    [matrix, patch],
  );

  // The waterfall's flows: every other Series component at the selected `t`,
  // with the selected component closing the chart as the Output-styled total.
  const flows = useMemo<Flow[]>(() => {
    if (!matrix || route.t === undefined) return [];
    const col = matrix.periods.indexOf(route.t);
    if (col < 0) return [];
    const byId = new Map(seriesNodes.map((n) => [n.id, n]));
    const out: Flow[] = [];
    matrix.components.forEach((component, row) => {
      const value = matrix.values[row][col];
      if (value === null) return;
      const node = byId.get(component);
      const selected = component === route.component;
      out.push({
        component,
        value,
        timing: node?.timing,
        unit: node?.unit,
        kind: (selected ? "Output" : node?.kind) as Flow["kind"],
        declarationIndex: node?.declaration_index ?? row,
      });
    });
    return out;
  }, [matrix, route.t, route.component, seriesNodes]);

  const toggleCollapsed = useCallback(
    (path: string) => {
      const set = new Set(route.collapsed);
      if (set.has(path)) set.delete(path);
      else set.add(path);
      patch({ collapsed: [...set] });
    },
    [patch, route.collapsed],
  );

  if (error) return <p className="err">cannot reach the run: {error}</p>;
  if (!route.mp) return <p className="err">no modelpoint selected</p>;
  if (!graph || !matrix) return <p>loading modelpoint {route.mp}…</p>;

  return (
    <main className="drilldown">
      <nav className="crumbs" aria-label="drill-down">
        {breadcrumbs(route).map((crumb, i) => (
          <button key={i} type="button" onClick={() => onRouteChange(crumb.route)}>
            {crumb.label}
          </button>
        ))}
      </nav>

      <div className="drilldown-toolbar">
        <label>
          <input type="checkbox" checked={route.heat} onChange={() => patch({ heat: !route.heat })} />
          heat
        </label>
        <label>
          <input type="checkbox" checked={showTable} onChange={() => setShowTable(!showTable)} />
          chart as table
        </label>
      </div>

      <ModelpointGrid
        matrix={matrix}
        selection={selection}
        onSelectionChange={onSelectionChange}
        onOpenCell={(cell) =>
          patch({ component: matrix.components[cell.row], t: matrix.periods[cell.col] })
        }
        heat={route.heat}
      />

      <div className="drilldown-lower">
        {route.t === undefined ? null : (
          <Waterfall
            flows={flows}
            t={route.t}
            sortByMagnitude={route.sortByMagnitude}
            onToggleSort={() => patch({ sortByMagnitude: !route.sortByMagnitude })}
            onSelect={(component) => patch({ component })}
            highlight={highlight}
            showTable={showTable}
          />
        )}
        {trace ? (
          <TracePanel
            trace={trace}
            collapsed={new Set(route.collapsed)}
            onToggle={toggleCollapsed}
            onNavigate={(component, t) => patch({ component, t })}
            onHighlight={setHighlight}
          />
        ) : (
          <p className="trace-empty">
            {traceError ? `explain() unavailable: ${traceError}` : "select a cell to explain it"}
          </p>
        )}
      </div>
    </main>
  );
}

/** URL-as-state (§5.1): the route lives in `location.search`, back button included. */
export function useDrilldownRoute(): [DrilldownRoute, (next: DrilldownRoute) => void] {
  const [route, setRoute] = useState(() => parseRoute(location.search));
  useEffect(() => {
    const onPop = () => setRoute(parseRoute(location.search));
    addEventListener("popstate", onPop);
    return () => removeEventListener("popstate", onPop);
  }, []);
  const push = useCallback((next: DrilldownRoute) => {
    history.pushState(null, "", formatRoute(next) + location.hash);
    setRoute(next);
  }, []);
  return [route, push];
}
