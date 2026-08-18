// The node inspector — 05-viz.md §2.3, §4.1.
//
// Full metadata, the `expr` with every identifier a link to its own node, the
// span it was declared at, its lints, its Prophet provenance line, and — when a
// run is loaded — a sparkline. With no run loaded this is *not* an error state:
// the panel says how to load one.

import type { ComponentDoc, GraphNode } from "./datasource";
import type { Impact } from "./graph/model";

/** Split an expression into identifier and non-identifier runs. Pure; tested. */
export function tokenizeExpr(expr: string): { text: string; identifier: boolean }[] {
  const out: { text: string; identifier: boolean }[] = [];
  const re = /[A-Za-z_][A-Za-z0-9_]*/g;
  let last = 0;
  for (let m = re.exec(expr); m !== null; m = re.exec(expr)) {
    if (m.index > last) out.push({ text: expr.slice(last, m.index), identifier: false });
    out.push({ text: m[0], identifier: true });
    last = m.index + m[0].length;
  }
  if (last < expr.length) out.push({ text: expr.slice(last), identifier: false });
  return out;
}

/** Keywords that look like identifiers but name no component. */
const KEYWORDS = new Set(["if", "then", "else", "and", "or", "not", "true", "false", "t"]);

export function Sparkline({ values }: { values: number[] }) {
  if (values.length < 2) return null;
  const min = Math.min(...values);
  const max = Math.max(...values);
  const span = max - min || 1;
  const points = values
    .map((v, i) => `${(i / (values.length - 1)) * 100},${24 - ((v - min) / span) * 22}`)
    .join(" ");
  return (
    <svg className="sparkline" data-testid="sparkline" viewBox="0 0 100 26" preserveAspectRatio="none">
      <polyline points={points} />
    </svg>
  );
}

export function Inspector({
  doc,
  impact,
  series,
  modelpoint,
  onNavigate,
  onTrace,
  hasRun,
  resolves,
}: {
  doc: ComponentDoc | null;
  impact?: Impact;
  series?: number[];
  modelpoint?: string;
  onNavigate: (name: string) => void;
  onTrace?: (t: number) => void;
  hasRun: boolean;
  resolves: (name: string) => boolean;
}) {
  if (!doc) {
    return (
      <aside className="inspector empty" data-testid="inspector">
        <p>Select a component to inspect it.</p>
      </aside>
    );
  }
  const node: GraphNode = doc.node;
  return (
    <aside className="inspector" data-testid="inspector" data-id={node.id}>
      <header>
        <h2>{node.name}</h2>
        <p className="meta">
          {node.kind} · {node.shape} · {node.dtype}
        </p>
        <p className="meta">
          unit {node.unit}
          {node.timing ? ` · ${node.timing}` : ""} · stage {node.stage} · depth {node.depth}
        </p>
        {node.retention && (
          <p className="meta">
            retention {node.retention}
            {node.hoistable ? " · hoisted" : ""}
          </p>
        )}
      </header>

      {node.doc && <p className="doc">{node.doc}</p>}

      {node.init !== undefined && (
        <section>
          <h3>init</h3>
          <pre className="code" data-testid="init">
            {node.init}
          </pre>
        </section>
      )}

      {node.expr !== undefined && (
        <section>
          <h3>expr</h3>
          <pre className="code" data-testid="expr">
            {tokenizeExpr(node.expr).map((tok, i) =>
              tok.identifier && !KEYWORDS.has(tok.text) && resolves(tok.text) ? (
                <button
                  key={i}
                  className="ident"
                  data-testid="expr-link"
                  data-name={tok.text}
                  onClick={() => onNavigate(tok.text)}
                >
                  {tok.text}
                </button>
              ) : (
                <span key={i}>{tok.text}</span>
              ),
            )}
          </pre>
        </section>
      )}

      {node.lints && node.lints.length > 0 && (
        <section data-testid="lints">
          <h3>lints</h3>
          <ul className="lints">
            {node.lints.map((lint) => (
              <li key={lint.code}>
                <code>{lint.code}</code> {lint.message}
              </li>
            ))}
          </ul>
        </section>
      )}

      {node.source?.variable && (
        <section data-testid="provenance">
          <h3>from Prophet</h3>
          <p className="prophet">
            {[node.source.library, node.source.variable].filter(Boolean).join(".")}
          </p>
          {node.source.file && <p className="meta">{node.source.file}</p>}
        </section>
      )}

      {node.span && (
        <section>
          <h3>declared at</h3>
          <p className="meta" data-testid="span">
            {node.span.file}:{node.span.line}:{node.span.col}
          </p>
        </section>
      )}

      <section>
        <h3>dependencies</h3>
        <p className="meta">
          reads {doc.upstream.length} · read by {doc.downstream.length}
        </p>
        <ul className="neighbours">
          {doc.upstream.map((id) => (
            <li key={`u${id}`}>
              <button data-testid="upstream" onClick={() => onNavigate(id)}>
                ← {id}
              </button>
            </li>
          ))}
          {doc.downstream.map((id) => (
            <li key={`d${id}`}>
              <button data-testid="downstream" onClick={() => onNavigate(id)}>
                → {id}
              </button>
            </li>
          ))}
        </ul>
      </section>

      {impact && impact.seed === node.id && (
        <section data-testid="impact-summary">
          <h3>impact</h3>
          <p>
            changing <code>{node.name}</code> affects {impact.components} components,{" "}
            {impact.outputs} of them Outputs.
          </p>
        </section>
      )}

      <section>
        <h3>values</h3>
        {hasRun && series && series.length > 0 ? (
          <>
            <p className="meta">{modelpoint}</p>
            <Sparkline values={series} />
            {onTrace && (
              <button className="trace" data-testid="trace" onClick={() => onTrace(0)}>
                trace at t = 0
              </button>
            )}
          </>
        ) : (
          <p className="meta" data-testid="no-run">
            load a run to see values: <code>results = model.run(...); results.show()</code>
          </p>
        )}
      </section>
    </aside>
  );
}
