import { useEffect, useMemo, useState } from "react";
import type { DataSource, Finding, GraphDoc, GraphNode, RunDiffDoc } from "../datasource";
import { isUnsupported } from "../datasource";
import { indexGraph, type GraphIndex } from "../graph/model";
import { matrixFromArrow } from "../drilldown/grid";
import type { Trace } from "../drilldown/trace";
import { AttributionPanel } from "./AttributionPanel";
import { ContributorBars } from "./ContributorBars";
import { ManifestBanner } from "./ManifestBanner";
import { SideBySide } from "./SideBySide";
import { orderFindings, shortName } from "./attribution";
import { detectCohort, type CohortResult, type MpFields } from "./cohort";
import {
  frameOf,
  rankContributors,
  toleranceFor,
  type ContributorRanking,
} from "./contributors";
import { compareMatrices, type MatrixComparison } from "./align";
import "./rundiff.css";

export interface RunDiffScreenProps {
  source: DataSource;
  /** Run ids, as `predictable diff run a b` names them. */
  a: string;
  b: string;
  /** Periods to pull for the side-by-side grid. */
  tFrom?: number;
  tTo?: number;
  /** Clicking "show in graph" hands the component back to the shell (§5.1). */
  onShowInGraph?: (component: string) => void;
}

/**
 * Screen 2 — Run Diff (05-viz.md §4.2).
 *
 * The migration loop, on one page: reconcile the manifests, read the movement,
 * read *why* it moved from the model diff, find the modelpoints carrying it and
 * the cohort they share, then open one of them side by side with both
 * `explain()` traces.
 *
 * Two rules hold throughout. The diff document is rendered **verbatim** — no
 * delta is recomputed and no finding is re-ranked here. Anything the screen
 * does derive (per-modelpoint contributions, the cohort) is labelled as derived
 * and carries its method.
 */
export function RunDiffScreen({
  source,
  a,
  b,
  tFrom = 0,
  tTo = 40,
  onShowInGraph,
}: RunDiffScreenProps) {
  const [doc, setDoc] = useState<RunDiffDoc | null>(null);
  const [graph, setGraph] = useState<GraphDoc | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [ranking, setRanking] = useState<ContributorRanking | null>(null);
  const [rankingError, setRankingError] = useState<string | null>(null);
  const [cohort, setCohort] = useState<CohortResult | null>(null);
  const [mp, setMp] = useState<string | null>(null);
  const [comparison, setComparison] = useState<MatrixComparison | null>(null);
  const [traceA, setTraceA] = useState<Trace | null>(null);
  const [traceB, setTraceB] = useState<Trace | null>(null);

  useEffect(() => {
    let live = true;
    setDoc(null);
    setError(null);
    source
      .diff(a, b)
      .then((d) => live && setDoc(d))
      .catch((e: Error) =>
        live &&
        setError(
          isUnsupported(e)
            ? "this source cannot diff runs: export the pack with `--include diff:<run>`"
            : e.message,
        ),
      );
    return () => {
      live = false;
    };
  }, [source, a, b]);

  useEffect(() => {
    let live = true;
    source
      .graph()
      .then((g) => live && setGraph(g))
      .catch(() => live && setGraph(null));
    return () => {
      live = false;
    };
  }, [source]);

  const index: GraphIndex | null = useMemo(() => (graph ? indexGraph(graph) : null), [graph]);
  const findings = useMemo(() => (doc ? orderFindings(doc.findings) : []), [doc]);
  const finding: Finding | null = useMemo(
    () => findings.find((f) => f.id === selectedId) ?? findings[0] ?? null,
    [findings, selectedId],
  );
  const tolerance = useMemo(
    () => (doc && finding ? toleranceFor(doc.tolerance, finding.component) : 0),
    [doc, finding],
  );

  // Contributor bars: the per-modelpoint movement of the selected finding's
  // component, from both runs' own series. The diff carries an exemplar and a
  // worst case, not every cell — so this is fetched, not read off the document.
  useEffect(() => {
    if (!doc || !finding) return;
    let live = true;
    setRanking(null);
    setRankingError(null);
    const component = finding.component;
    Promise.all([
      source.series({ run: doc.a.run, components: [component], tFrom, tTo }),
      source.series({ run: doc.b.run, components: [component], tFrom, tTo }),
    ])
      .then(([ta, tb]) => {
        if (!live) return;
        setRanking(
          rankContributors(component, frameOf(ta, component), frameOf(tb, component), tolerance),
        );
      })
      .catch((e: Error) => live && setRankingError(e.message));
    return () => {
      live = false;
    };
  }, [source, doc, finding, tolerance, tFrom, tTo]);

  // Cohort detection needs the declared modelpoint fields. They come over the
  // same `/api/series` wire, which is `Float64`-valued — so string-valued
  // fields (`sex`) cannot be split on here and are simply not offered.
  useEffect(() => {
    if (!graph || !doc || !ranking) return;
    let live = true;
    const fieldNodes: GraphNode[] = graph.nodes.filter(
      (n) => n.kind === "InputModelpoint" && n.dtype !== "str" && n.dtype !== "date",
    );
    if (fieldNodes.length === 0) {
      setCohort(detectCohort(new Map(), ranking.differing, ranking.matching));
      return;
    }
    const ids = fieldNodes.map((n) => n.id);
    source
      .series({ run: doc.b.run, components: ids, tFrom: 0, tTo: 0 })
      .then((table) => {
        if (!live) return;
        const fields: MpFields = new Map();
        for (const id of ids) {
          try {
            const frame = frameOf(table, id);
            const values = new Map<string, number>();
            frame.mp.forEach((key, i) => {
              const v = frame.values[i];
              if (v !== null && Number.isFinite(v)) values.set(key, v);
            });
            if (values.size > 0) fields.set(id, values);
          } catch {
            // A field the source cannot serve is a field we do not split on.
          }
        }
        setCohort(detectCohort(fields, ranking.differing, ranking.matching));
      })
      .catch(() => live && setCohort(detectCohort(new Map(), ranking.differing, ranking.matching)));
    return () => {
      live = false;
    };
  }, [source, graph, doc, ranking]);

  // Side-by-side: both runs' grids for one modelpoint, plus both traces.
  useEffect(() => {
    if (!doc || !mp || !graph || !finding) return;
    let live = true;
    setComparison(null);
    setTraceA(null);
    setTraceB(null);
    const components = graph.nodes
      .filter((n) => n.shape === "Series")
      .slice()
      .sort((x, y) => x.declaration_index - y.declaration_index)
      .map((n) => n.id);
    const wanted = components.length > 0 ? components : [finding.component];
    Promise.all([
      source.series({ run: doc.a.run, components: wanted, modelpoints: [mp], tFrom, tTo }),
      source.series({ run: doc.b.run, components: wanted, modelpoints: [mp], tFrom, tTo }),
    ])
      .then(([ta, tb]) => {
        if (!live) return;
        setComparison(
          compareMatrices(matrixFromArrow(ta, wanted), matrixFromArrow(tb, wanted), tolerance),
        );
      })
      .catch(() => live && setComparison(null));

    const t = finding.t_first;
    source
      .explain({ run: doc.a.run, component: finding.component, modelpoint: mp, t })
      .then((d) => live && setTraceA(d as Trace))
      .catch(() => live && setTraceA(null));
    source
      .explain({ run: doc.b.run, component: finding.component, modelpoint: mp, t })
      .then((d) => live && setTraceB(d as Trace))
      .catch(() => live && setTraceB(null));
    return () => {
      live = false;
    };
  }, [source, doc, graph, mp, finding, tolerance, tFrom, tTo]);

  if (error) return <p className="rd-err">cannot load the diff: {error}</p>;
  if (!doc) return <p className="rd-loading">loading the diff…</p>;

  return (
    <div className="rundiff">
      <ManifestBanner doc={doc} />

      <div className="rd-body">
        <nav className="rd-findings" aria-label="findings">
          <h2>
            {doc.findings.length} findings
            {doc.findings_truncated &&
              ` (${doc.findings_truncated.kept} of ${doc.findings_truncated.of} kept)`}
          </h2>
          <ul>
            {findings.map((f) => (
              <li key={f.id}>
                <button
                  type="button"
                  data-finding={f.id}
                  aria-current={finding?.id === f.id}
                  className={finding?.id === f.id ? "rd-finding rd-selected" : "rd-finding"}
                  onClick={() => {
                    setSelectedId(f.id);
                    setMp(null);
                  }}
                >
                  <span className="rd-id">{f.id}</span>
                  <span className={`rd-class rd-class-${f.class}`}>{f.class}</span>
                  <span className="rd-component">{shortName(f.component)}</span>
                  {f.contribution && (
                    <span className="rd-share">
                      {(f.contribution.share_of_total_delta * 100).toFixed(0)}% of Δ
                      {shortName(f.contribution.output)}
                    </span>
                  )}
                </button>
              </li>
            ))}
          </ul>
          {(doc.structural.only_in_a.length > 0 || doc.structural.only_in_b.length > 0) && (
            <p className="rd-note" data-testid="structural">
              only in A: {doc.structural.only_in_a.map((o) => o.component).join(", ") || "—"} · only
              in B: {doc.structural.only_in_b.map((o) => o.component).join(", ") || "—"}
            </p>
          )}
        </nav>

        {finding && (
          <AttributionPanel
            doc={doc}
            finding={finding}
            index={index}
            onShowInGraph={onShowInGraph}
            onOpenSideBySide={setMp}
          />
        )}

        <div className="rd-right">
          {rankingError && <p className="rd-note">contributors unavailable: {rankingError}</p>}
          {ranking ? (
            <ContributorBars ranking={ranking} cohort={cohort} onOpen={setMp} />
          ) : (
            !rankingError && <p className="rd-loading">ranking modelpoints…</p>
          )}
        </div>
      </div>

      {mp && (
        <SideBySide
          mp={mp}
          comparison={comparison}
          traceA={traceA}
          traceB={traceB}
          tolerance={tolerance}
          onClose={() => setMp(null)}
        />
      )}
    </div>
  );
}
