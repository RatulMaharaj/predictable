// Screen 1 — the Model Explorer (05-viz.md §4.1).
//
// Layout is the spec's: module tree and filters on the left, the graph in the
// middle, the inspector on the right, `⌘K` over everything. Every piece of state
// this screen holds is in the URL (§5.1), and nothing on this screen computes an
// actuarial number or a topological order — it renders the plan the engine sent.

import { useEffect, useMemo, useState } from "react";
import type { Capabilities, ComponentDoc, DataSource, GraphDoc } from "./datasource";
import { Inspector } from "./Inspector";
import { Palette } from "./Palette";
import { CanvasRenderer } from "./graph/CanvasRenderer";
import { SvgRenderer } from "./graph/SvgRenderer";
import { buildScene } from "./graph/scene";
import { facets, impact as impactOf, indexGraph, resolveId } from "./graph/model";
import { rendererFor } from "./graph/style";
import { LayoutCache } from "./graph/layout";
import { useLayout, type LayoutEngine } from "./graph/useLayout";
import { useView } from "./store";

/** `sha256:46e106…` → `46e106e`. The algorithm prefix is noise in a header. */
export function shortDigest(digest: string): string {
  const colon = digest.indexOf(":");
  return (colon === -1 ? digest : digest.slice(colon + 1)).slice(0, 7);
}

function Facet({
  label,
  values,
  active,
  onToggle,
}: {
  label: string;
  values: string[];
  active: string[];
  onToggle: (value: string) => void;
}) {
  if (values.length === 0) return null;
  return (
    <div className="facet" data-testid={`facet-${label}`}>
      <h3>{label}</h3>
      <ul>
        {values.map((value) => (
          <li key={value}>
            <label>
              <input
                type="checkbox"
                checked={active.includes(value)}
                onChange={() => onToggle(value)}
              />
              {value}
            </label>
          </li>
        ))}
      </ul>
    </div>
  );
}

export function App({
  source,
  engine,
  cache = LayoutCache.browser(),
}: {
  source: DataSource;
  engine: LayoutEngine;
  cache?: LayoutCache;
}) {
  const [graph, setGraph] = useState<GraphDoc | null>(null);
  const [caps, setCaps] = useState<Capabilities | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selectedDoc, setSelectedDoc] = useState<ComponentDoc | null>(null);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const view = useView();

  useEffect(() => {
    Promise.all([source.graph(), source.capabilities()])
      .then(([g, c]) => {
        setGraph(g);
        setCaps(c);
      })
      .catch((e: Error) => setError(e.message));
  }, [source]);

  const index = useMemo(() => (graph ? indexGraph(graph) : null), [graph]);
  const layoutState = useLayout(graph, engine, cache);

  // The inspector reads the *source's* component document, not a local slice of
  // the graph: `upstream`/`downstream` must be the planner's answer (§2.1).
  useEffect(() => {
    if (!view.selected) {
      setSelectedDoc(null);
      return;
    }
    let live = true;
    source
      .component(view.selected)
      .then((d) => live && setSelectedDoc(d))
      .catch(() => live && setSelectedDoc(null));
    return () => {
      live = false;
    };
  }, [source, view.selected]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "k" && (event.metaKey || event.ctrlKey)) {
        event.preventDefault();
        setPaletteOpen((open) => !open);
      } else if (event.key === "Escape") {
        setPaletteOpen(false);
      } else if (event.key === "i" && !event.metaKey && !event.ctrlKey && view.selected) {
        const target = event.target as HTMLElement | null;
        if (target && (target.tagName === "INPUT" || target.tagName === "TEXTAREA")) return;
        useView.getState().toggle("impact");
      }
    };
    globalThis.addEventListener("keydown", onKey);
    return () => globalThis.removeEventListener("keydown", onKey);
  }, [view.selected]);

  if (error) return <p className="err">cannot reach the run: {error}</p>;
  if (!graph || !index || !caps) return <p className="loading">connecting…</p>;

  const scene =
    layoutState.status === "cached" || layoutState.status === "ready"
      ? buildScene(index, layoutState.layout, {
          filters: view.filters,
          selected: view.selected,
          focus: view.focus,
          impact: view.impact,
          layers: view.layers,
        })
      : null;

  const impact = view.selected ? impactOf(index, view.selected) : undefined;
  const facetValues = facets(graph);
  const renderer = scene ? rendererFor(scene.nodes.length) : "svg";
  const select = (id: string) => useView.getState().select(id);
  const navigate = (name: string) => {
    const id = resolveId(index, name);
    if (id) select(id);
  };

  return (
    <div className="explorer">
      <header className="topbar">
        <strong>predictable</strong>
        <span className="modules">{graph.modules.join(" · ")}</span>
        <span className="digest" title={graph.model_digest}>
          model {shortDigest(graph.model_digest)}
        </span>
        <span className="digest" title={graph.order_digest}>
          order {shortDigest(graph.order_digest)}
        </span>
        <button className="kbd" onClick={() => setPaletteOpen(true)} aria-label="open command palette">
          ⌘K
        </button>
      </header>

      <nav className="sidebar">
        <Facet
          label="module"
          values={facetValues.modules}
          active={view.filters.modules}
          onToggle={(v) => useView.getState().toggleFacet("modules", v)}
        />
        <Facet
          label="unit"
          values={facetValues.units}
          active={view.filters.units}
          onToggle={(v) => useView.getState().toggleFacet("units", v)}
        />
        <Facet
          label="kind"
          values={facetValues.kinds}
          active={view.filters.kinds}
          onToggle={(v) => useView.getState().toggleFacet("kinds", v)}
        />
        <Facet
          label="tag"
          values={facetValues.tags}
          active={view.filters.tags}
          onToggle={(v) => useView.getState().toggleFacet("tags", v)}
        />

        <div className="overlays">
          <h3>overlays</h3>
          <label>
            <input
              type="checkbox"
              checked={view.impact}
              onChange={() => useView.getState().toggle("impact")}
            />
            impact
          </label>
          <label>
            <input
              type="checkbox"
              checked={view.focus}
              onChange={() => useView.getState().toggle("focus")}
            />
            focus
          </label>
          <label>
            <input
              type="checkbox"
              checked={view.layers}
              onChange={() => useView.getState().toggle("layers")}
            />
            layers
          </label>
        </div>

        <p className="counts" data-testid="counts">
          {scene ? scene.nodes.length : graph.nodes.length} nodes ·{" "}
          {scene ? scene.edges.length : graph.edges.length} edges · {graph.layers.length} layers
        </p>
        {view.impact && impact && (
          <p className="impact-count" data-testid="impact-count">
            {impact.components} impacted, {impact.outputs} Outputs
          </p>
        )}
        <p className="renderer" data-testid="renderer">
          renderer {renderer}
        </p>
      </nav>

      <main className="canvas-area">
        {layoutState.status === "laying-out" && (
          <p className="loading" data-testid="laying-out">
            laying out {graph.nodes.length} components…
          </p>
        )}
        {layoutState.status === "error" && (
          <p className="err" data-testid="layout-error">
            layout failed: {layoutState.message}
          </p>
        )}
        {scene &&
          (renderer === "canvas" ? (
            <CanvasRenderer
              scene={scene}
              onSelect={select}
              onFocus={(id) => useView.getState().set({ selected: id, focus: true })}
            />
          ) : (
            <SvgRenderer
              scene={scene}
              onSelect={select}
              onFocus={(id) => useView.getState().set({ selected: id, focus: true })}
            />
          ))}
      </main>

      <Inspector
        doc={selectedDoc}
        impact={view.impact ? impact : undefined}
        hasRun={caps.recompute || caps.explain}
        modelpoint={view.modelpoint}
        onNavigate={navigate}
        resolves={(name) => resolveId(index, name) !== undefined}
      />

      <Palette
        doc={graph}
        open={paletteOpen}
        initialQuery={view.query}
        onClose={() => setPaletteOpen(false)}
        onPick={(id, query) => {
          useView.getState().set({ selected: id, query });
          setPaletteOpen(false);
        }}
      />
    </div>
  );
}
