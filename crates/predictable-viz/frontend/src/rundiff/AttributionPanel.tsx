import type { Finding, RunDiffDoc } from "../datasource";
import type { GraphIndex } from "../graph/model";
import { attribution, shortName } from "./attribution";

interface SuggestedEdit {
  file?: string;
  byte_start?: number;
  byte_end?: number;
  old?: string;
  new?: string;
  message?: string;
}

/**
 * The attribution panel (05-viz.md §4.2, left).
 *
 * It answers "why", not "what": the model diff's own classification of the
 * edit, the impact set that edit reaches, and the hypotheses the run diff
 * attached with their evidence support. Everything on it comes out of
 * `diff.json`; the panel adds no judgement of its own.
 */
export function AttributionPanel({
  doc,
  finding,
  index,
  onShowInGraph,
  onOpenSideBySide,
}: {
  doc: RunDiffDoc;
  finding: Finding;
  index?: GraphIndex | null;
  onShowInGraph?: (component: string) => void;
  onOpenSideBySide?: (mp: string) => void;
}) {
  const model = attribution(doc, finding, index);

  return (
    <section className="rd-attribution" aria-label="attribution">
      <header>
        <h2>
          <span className={`rd-class rd-class-${finding.class}`}>{finding.class}</span>{" "}
          <code>{shortName(finding.component)}</code>
        </h2>
        <p className="rd-sentence" data-testid="attribution-sentence">
          {model.sentence}
        </p>
      </header>

      {model.available && model.changed && (
        <p className="rd-what" data-testid="attribution-what">
          model diff: {model.what.join(", ")}
        </p>
      )}
      {!model.available && (
        <p className="rd-note" data-testid="attribution-silent">
          model attribution unavailable
        </p>
      )}

      {model.impact && (
        <p className="rd-impact" data-testid="attribution-impact">
          impact set: {model.impact.components} components, reaching{" "}
          {model.impact.outputs.map(shortName).join(", ") || "no Output"}
        </p>
      )}
      <p className="rd-affects">
        diverging outputs: {finding.affects_outputs.map(shortName).join(", ")}
      </p>
      <p className="rd-extent">
        first at t={finding.t_first}, persists to t={finding.t_range[1]} ·{" "}
        {finding.n_modelpoints} modelpoints · {finding.n_cells} cells
        {finding.contribution &&
          ` · ${(finding.contribution.share_of_total_delta * 100).toFixed(0)}% of Δ${shortName(
            finding.contribution.output,
          )} (${finding.contribution.method})`}
      </p>

      <dl className="rd-exemplars">
        <dt>exemplar</dt>
        <dd data-testid="exemplar">
          {finding.exemplar.mp_key} t={finding.exemplar.t} {String(finding.exemplar.a)} vs{" "}
          {String(finding.exemplar.b)}
        </dd>
        <dt>worst</dt>
        <dd data-testid="worst">
          {finding.worst.mp_key} t={finding.worst.t} {String(finding.worst.a)} vs{" "}
          {String(finding.worst.b)}
        </dd>
      </dl>

      <ul className="rd-hypotheses" aria-label="hypotheses">
        {finding.hypotheses.map((h) => {
          const edit = h.suggested_edit as SuggestedEdit | null;
          return (
            <li key={h.code} className="rd-hypothesis" data-code={h.code}>
              <p>
                <span className="rd-code">{h.code}</span>{" "}
                <span className={`rd-confidence rd-confidence-${h.confidence}`}>
                  {h.confidence} confidence
                </span>{" "}
                <span className="rd-support">
                  held on {h.evidence_support.held}/{h.evidence_support.tested}
                </span>
              </p>
              <p className="rd-hypothesis-message">{h.message}</p>
              {edit?.new && (
                <pre className="rd-edit" data-testid={`edit-${h.code}`}>
                  <span className="rd-edit-file">
                    {edit.file}:{edit.byte_start}..{edit.byte_end}
                  </span>
                  {"\n"}
                  <span className="rd-edit-old">- {edit.old}</span>
                  {"\n"}
                  <span className="rd-edit-new">+ {edit.new}</span>
                </pre>
              )}
            </li>
          );
        })}
        {finding.hypotheses.length === 0 && (
          <li className="rd-note">no hypothesis held well enough to be worth showing</li>
        )}
      </ul>

      <p className="rd-actions">
        <button type="button" onClick={() => onShowInGraph?.(finding.component)}>
          show in graph
        </button>
        <button type="button" onClick={() => onOpenSideBySide?.(finding.exemplar.mp_key)}>
          open {finding.exemplar.mp_key} side-by-side
        </button>
      </p>
      <pre className="rd-command" data-testid="explain-command">
        {finding.explain_command}
      </pre>
    </section>
  );
}
