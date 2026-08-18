import {
  flattenTrace,
  formatExact,
  formatValue,
  nodeBadges,
  nodeLabel,
  notesByPath,
  spanLabel,
  type Trace,
  type TraceNode,
} from "./trace";

export interface TracePanelProps {
  trace: Trace;
  /** Paths the reader folded shut; lives in the URL (§5.1). */
  collapsed: ReadonlySet<string>;
  onToggle: (path: string) => void;
  /** A `Ref` navigates to that component's own trace at that `t`. */
  onNavigate?: (component: string, t: number) => void;
  /** A span opens the `.pir` at that line. */
  onOpenSpan?: (node: TraceNode) => void;
  /** A `Lookup` opens the table viewer at the resolved row. */
  onOpenTable?: (table: string, row: number) => void;
  /** Clicking a trace node highlights the matching waterfall bar. */
  onHighlight?: (component: string | null) => void;
}

const REF_KINDS = new Set(["Ref", "Lag", "At", "Component"]);

/**
 * The `explain()` panel. It renders the trace tree of 04-verify.md §3.2
 * **verbatim** — the engine's node order, the engine's values, the engine's
 * notes — and adds only affordances: expand, navigate, open a span, open a
 * table row.
 *
 * Nothing here computes a number. If a value is on screen, the engine put it in
 * the JSON.
 */
export function TracePanel({
  trace,
  collapsed,
  onToggle,
  onNavigate,
  onOpenSpan,
  onOpenTable,
  onHighlight,
}: TracePanelProps) {
  const rows = flattenTrace(trace.root, collapsed);
  const notes = notesByPath(trace.notes);
  const root = trace.root;

  return (
    <section className="trace-panel" aria-label="explain trace">
      <header className="trace-head">
        <h2>
          explain <code>{root.id ?? root.ref}</code>
          {root.modelpoint ? ` · ${root.modelpoint.key}` : ""}
          {root.t === undefined ? "" : ` · t = ${root.t}`}
        </h2>
        <p className="trace-value" title={formatExact(root.value)}>
          = {formatValue(root.value)} {root.unit ?? ""}
          {root.timing ? ` (${root.timing})` : ""}
        </p>
        {root.expr ? <pre className="trace-expr">{root.expr}</pre> : null}
      </header>

      <ol className="trace-tree">
        {rows.map((row) => {
          const node = row.node;
          const badges = nodeBadges(node);
          const rowNotes = notes.get(node.path) ?? [];
          const warn = badges.some((b) => b.tone === "warn") || rowNotes.length > 0;
          const span = spanLabel(node.span);
          const canNavigate =
            REF_KINDS.has(node.node) && node.ref !== undefined && node.t !== undefined;
          return (
            <li
              key={node.path}
              className={`trace-row${warn ? " is-warn" : ""}`}
              data-path={node.path}
              data-node={node.node}
              style={{ paddingLeft: `${row.depth * 1.1}rem` }}
              onMouseEnter={() => onHighlight?.(node.ref ?? node.id ?? null)}
              onMouseLeave={() => onHighlight?.(null)}
            >
              <span className="trace-line">
                {node.children?.length ? (
                  <button
                    type="button"
                    className="trace-toggle"
                    aria-expanded={!row.collapsed}
                    aria-label={`${row.collapsed ? "expand" : "collapse"} ${node.path}`}
                    onClick={() => onToggle(node.path)}
                  >
                    {row.collapsed ? "▸" : "▾"}
                  </button>
                ) : (
                  <span className="trace-toggle is-leaf">·</span>
                )}
                {canNavigate ? (
                  <button
                    type="button"
                    className="trace-ref"
                    onClick={() => onNavigate?.(node.ref!, node.t!)}
                  >
                    {nodeLabel(node)}
                  </button>
                ) : (
                  <span className="trace-label">{nodeLabel(node)}</span>
                )}
                <span className="trace-num" title={formatExact(node.value)}>
                  {node.text ?? formatValue(node.value)}
                </span>
                {node.unit ? <span className="trace-unit">{node.unit}</span> : null}
                {node.timing ? <span className="trace-timing">{node.timing}</span> : null}
                {span ? (
                  <button type="button" className="trace-span" onClick={() => onOpenSpan?.(node)}>
                    {span}
                  </button>
                ) : null}
                {node.node === "Lookup" && node.table && node.row !== undefined ? (
                  <button
                    type="button"
                    className="trace-table"
                    onClick={() => onOpenTable?.(node.table!, node.row!)}
                  >
                    open table at row {node.row}
                  </button>
                ) : null}
              </span>

              {badges.map((badge, i) => (
                <span key={i} className={`trace-badge is-${badge.tone}`} title={badge.title}>
                  {badge.text}
                </span>
              ))}
              {rowNotes.map((note) => (
                <span key={note.code + note.path} className={`trace-note is-${note.severity}`}>
                  {note.code} {note.message}
                </span>
              ))}

              {node.node === "Agg" && node.terms?.length ? (
                <table className="trace-terms">
                  <thead>
                    <tr>
                      <th>t</th>
                      <th>x</th>
                      <th>disc</th>
                      <th>contribution</th>
                    </tr>
                  </thead>
                  <tbody>
                    {node.terms.map((term) => (
                      <tr key={term.t} className={term.included === false ? "is-excluded" : ""}>
                        <td>{term.t}</td>
                        <td title={formatExact(term.value)}>{formatValue(term.value)}</td>
                        <td>{term.disc === undefined ? "—" : formatValue(term.disc)}</td>
                        <td title={formatExact(term.contribution)}>
                          {formatValue(term.contribution)}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              ) : null}
            </li>
          );
        })}
      </ol>

      {trace.truncated.elided_nodes > 0 ? (
        <p className="trace-truncated">
          truncated at depth {trace.truncated.depth}: {trace.truncated.elided_nodes} nodes elided
        </p>
      ) : null}
      {trace.run ? <p className="trace-run">run {trace.run.slice(0, 19)}</p> : null}
    </section>
  );
}
