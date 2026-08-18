// Getting a layout: cache first, worker second — 05-viz.md §2.2.
//
// The state machine is deliberately explicit rather than a bare promise, because
// §5.3 requires *progress* to be visible during a relayout and "instant" to be
// visible when the cache hit.

import { useEffect, useState } from "react";
import ELK from "elkjs/lib/elk-api.js";
import ElkWorker from "elkjs/lib/elk-worker.min.js?worker&inline";
import type { GraphDoc } from "../datasource";
import {
  LayoutCache,
  layoutCovers,
  toElkGraph,
  fromElkResult,
  type Layout,
  type LayoutRequest,
} from "./layout";

export type LayoutState =
  | { status: "cached"; layout: Layout }
  | { status: "laying-out" }
  | { status: "ready"; layout: Layout }
  | { status: "error"; message: string };

/** How the layout reaches the worker. Injectable so tests need no Worker. */
export interface LayoutEngine {
  layout(request: LayoutRequest): Promise<Layout>;
  dispose?(): void;
}

/**
 * The production engine: ELK's solver, in a Web Worker.
 *
 * Two details are load-bearing and were both found the hard way:
 *
 * - **`elk-api` + an explicit `workerFactory`, not `elk.bundled.js`.** The
 *   bundled build falls back to `require("./elk-worker.min.js")` at run time,
 *   which a browser bundle stubs out — the constructor then throws
 *   *"… is not a constructor"* and the graph never appears. Handing ELK its
 *   worker explicitly removes the run-time `require` entirely.
 * - **`?worker&inline`.** §1.4 requires the whole app to be *one* file that
 *   fetches nothing; a worker emitted as a sibling `.js` would be fetched at
 *   run time, so a governance pack opened from a network share would silently
 *   lose its layout. Inlined, the worker is a blob URL built from this bundle.
 *
 * The solver therefore never runs on the main thread, which is what §5.3's
 * "relayout < 2.5 s in a worker, with progress" is asking for.
 */
export function workerEngine(): LayoutEngine {
  const elk = new ELK({ workerFactory: () => new ElkWorker() });
  return {
    async layout(request) {
      const result = await elk.layout(request.graph as never);
      return fromElkResult(result, request.model_digest);
    },
  };
}

/**
 * Resolve a layout for `doc`, using `cache` when it already holds one for this
 * `model_digest` and asking `engine` otherwise. The result of a fresh layout is
 * written back to the cache, so the next session opens instantly and — the point
 * of §2.2 — with identical node positions.
 */
export async function resolveLayout(
  doc: GraphDoc,
  cache: LayoutCache,
  engine: LayoutEngine,
): Promise<{ layout: Layout; fromCache: boolean }> {
  const hit = cache.get(doc.model_digest);
  if (hit && layoutCovers(hit, doc)) return { layout: hit, fromCache: true };
  const layout = await engine.layout({
    kind: "layout",
    graph: toElkGraph(doc),
    model_digest: doc.model_digest,
  });
  cache.put(layout);
  return { layout, fromCache: false };
}

export function useLayout(
  doc: GraphDoc | null,
  engine: LayoutEngine,
  cache: LayoutCache = LayoutCache.browser(),
): LayoutState {
  const [state, setState] = useState<LayoutState>({ status: "laying-out" });

  useEffect(() => {
    if (!doc) return;
    let live = true;
    const hit = cache.get(doc.model_digest);
    if (hit && layoutCovers(hit, doc)) {
      setState({ status: "cached", layout: hit });
      return;
    }
    setState({ status: "laying-out" });
    resolveLayout(doc, cache, engine)
      .then(({ layout }) => live && setState({ status: "ready", layout }))
      .catch((error: Error) => live && setState({ status: "error", message: error.message }));
    return () => {
      live = false;
    };
    // `cache` is a stable handle over localStorage; the digest is the real key.
  }, [doc, engine]); // eslint-disable-line react-hooks/exhaustive-deps

  return state;
}
