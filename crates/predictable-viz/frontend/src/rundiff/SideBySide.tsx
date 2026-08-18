import { useEffect, useRef, useState } from "react";
import { TracePanel } from "../drilldown/TracePanel";
import { formatValue, type Trace } from "../drilldown/trace";
import { shortName } from "./attribution";
import { alignTraces, divergenceSentence, type MatrixComparison } from "./align";

/**
 * The side-by-side modelpoint view (05-viz.md §4.2, last bullet).
 *
 * Two column-aligned grids over the union of components and periods, differing
 * cells highlighted, and both `explain()` traces stacked. The first `t` at
 * which the grids disagree is scrolled into view and marked; the deepest node
 * at which the *trees* disagree is marked too, because that is the edit rather
 * than its consequence.
 */
export function SideBySide({
  mp,
  comparison,
  traceA,
  traceB,
  tolerance = 0,
  onClose,
}: {
  mp: string;
  comparison: MatrixComparison | null;
  traceA: Trace | null;
  traceB: Trace | null;
  tolerance?: number;
  onClose?: () => void;
}) {
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set());
  const [onlyDiffering, setOnlyDiffering] = useState(false);
  const marker = useRef<HTMLTableCellElement | null>(null);

  useEffect(() => {
    marker.current?.scrollIntoView?.({ inline: "center", block: "nearest" });
  }, [comparison, mp]);

  const alignment = traceA && traceB ? alignTraces(traceA, traceB, tolerance) : null;
  const toggle = (path: string) =>
    setCollapsed((set) => {
      const next = new Set(set);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });

  const rows = comparison
    ? comparison.components
        .map((component, i) => ({ component, i }))
        .filter(({ i }) => !onlyDiffering || comparison.cells[i].some((c) => c.differs))
    : [];

  return (
    <section className="rd-sidebyside" aria-label={`side by side ${mp}`}>
      <header>
        <h2>
          side by side · <code>{mp}</code>
        </h2>
        {comparison && (
          <span className="rd-sbs-summary" data-testid="sbs-summary">
            {comparison.differingCells} differing cells in {comparison.differingComponents.length}{" "}
            components
            {comparison.firstT === null ? "" : ` · first divergence at t=${comparison.firstT}`}
          </span>
        )}
        <label>
          <input
            type="checkbox"
            checked={onlyDiffering}
            onChange={() => setOnlyDiffering((v) => !v)}
          />
          differing rows only
        </label>
        {onClose && (
          <button type="button" onClick={onClose}>
            close
          </button>
        )}
      </header>

      {comparison && (
        <div className="rd-grid-scroll">
          <table className="rd-grid">
            <thead>
              <tr>
                <th scope="col">component</th>
                {comparison.periods.map((t) => (
                  <th
                    key={t}
                    scope="col"
                    className={t === comparison.firstT ? "rd-t rd-first-divergence" : "rd-t"}
                  >
                    t={t}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map(({ component, i }) => (
                <tr key={component} data-component={component}>
                  <th scope="row">{shortName(component)}</th>
                  {comparison.cells[i].map((cell, j) => (
                    <td
                      key={j}
                      className={cell.differs ? "rd-cell rd-cell-differs" : "rd-cell"}
                      data-differs={String(cell.differs)}
                      ref={
                        cell.differs && comparison.periods[j] === comparison.firstT && !marker.current
                          ? marker
                          : undefined
                      }
                    >
                      <span className="rd-cell-a">{cell.a === null ? "—" : formatValue(cell.a)}</span>
                      <span className="rd-cell-b">{cell.b === null ? "—" : formatValue(cell.b)}</span>
                      {cell.delta !== null && cell.differs && (
                        <span className={cell.delta >= 0 ? "rd-chip rd-pos" : "rd-chip rd-neg"}>
                          {cell.delta >= 0 ? "+" : "−"}
                          {formatValue(Math.abs(cell.delta))}
                        </span>
                      )}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {alignment && divergenceSentence(alignment) && (
        <p className="rd-divergence-note" data-testid="trace-divergence">
          {divergenceSentence(alignment)}
          {alignment.firstDivergence && (
            <span className="rd-divergence-values">
              {" "}
              · at the root A{" "}
              {alignment.firstDivergence.a ? formatValue(alignment.firstDivergence.a.value) : "—"} vs
              B{" "}
              {alignment.firstDivergence.b ? formatValue(alignment.firstDivergence.b.value) : "—"}
            </span>
          )}
        </p>
      )}

      <div className="rd-traces">
        <div className="rd-trace rd-trace-a">
          <h3>A · {traceA?.run ?? "—"}</h3>
          {traceA ? (
            <TracePanel trace={traceA} collapsed={collapsed} onToggle={toggle} />
          ) : (
            <p className="rd-note">no trace for side A</p>
          )}
        </div>
        <div className="rd-trace rd-trace-b">
          <h3>B · {traceB?.run ?? "—"}</h3>
          {traceB ? (
            <TracePanel trace={traceB} collapsed={collapsed} onToggle={toggle} />
          ) : (
            <p className="rd-note">no trace for side B</p>
          )}
        </div>
      </div>
    </section>
  );
}
