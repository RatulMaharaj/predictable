import type { RunDiffDoc } from "../datasource";
import { hasBlocking, headline, reconcile, shortDigest } from "./reconcile";

/**
 * The reconciliation banner (05-viz.md §4.2).
 *
 * It renders before anything numeric on the screen, and it renders even when
 * the verdict is `pass` — "everything matched *and* it was the same modelpoint
 * file" is the statement that ends an argument.
 */
export function ManifestBanner({ doc }: { doc: RunDiffDoc }) {
  const checks = reconcile(doc);
  const blocking = hasBlocking(checks);

  return (
    <section className="rd-banner" aria-label="manifest reconciliation">
      <header className="rd-sides">
        <div className="rd-side">
          <span className="rd-tag">A</span>
          <span className="rd-system">{doc.a.system}</span>
          <code>{doc.a.run}</code>
          <span className="rd-path">{doc.a.path}</span>
        </div>
        <div className="rd-side">
          <span className="rd-tag">B</span>
          <span className="rd-system">{doc.b.system}</span>
          <code>{doc.b.run}</code>
          <span className="rd-path">{doc.b.path}</span>
        </div>
        <span className={`rd-verdict rd-verdict-${doc.summary.verdict}`} data-testid="verdict">
          {doc.summary.verdict}
        </span>
      </header>

      <ul className="rd-checks">
        {checks.map((check) => (
          <li key={check.key} data-check={check.key} data-status={check.status} className="rd-check">
            <span className="rd-check-mark" aria-hidden="true">
              {check.status === "same" ? "✓" : check.status === "different" ? "✕" : "?"}
            </span>
            <span className="rd-check-label">{check.label}</span>
            <span className="rd-check-values">
              {check.status === "same" ? check.a : `${check.a} → ${check.b}`}
            </span>
            {check.note && <span className="rd-check-note">{check.note}</span>}
          </li>
        ))}
      </ul>

      {blocking && (
        <p className="rd-blocking" data-testid="blocking">
          the two runs were not given the same inputs — read the differences above before reading a
          single number below
        </p>
      )}

      <p className="rd-headline" data-testid="headline">
        {headline(doc)}
      </p>

      <ul className="rd-outputs" aria-label="output movements">
        {doc.summary.outputs.map((output) => {
          const worst = Math.max(
            ...doc.summary.outputs.map((o) => Math.abs(o.rel)),
            Number.MIN_VALUE,
          );
          const width = worst === 0 ? 0 : (Math.abs(output.rel) / worst) * 100;
          return (
            <li
              key={output.component}
              className={output.within_tolerance ? "rd-output rd-ok" : "rd-output rd-differs"}
              data-output={output.component}
              data-within={String(output.within_tolerance)}
            >
              <span className="rd-output-name">{output.component}</span>
              {output.within_tolerance ? (
                <span className="rd-exact">✓ exact</span>
              ) : (
                <>
                  <span className="rd-delta">
                    Δ {(output.b_total - output.a_total).toLocaleString("en-US", {
                      maximumFractionDigits: 2,
                    })}
                  </span>
                  <span className="rd-rel">({(output.rel * 100).toFixed(3)}%)</span>
                  <span className="rd-bar" style={{ width: `${width}%` }} aria-hidden="true" />
                </>
              )}
            </li>
          );
        })}
      </ul>

      <p className="rd-cells">
        {doc.summary.cells.diverged.toLocaleString("en-US")} of{" "}
        {doc.summary.cells.compared.toLocaleString("en-US")} cells diverged ·{" "}
        {doc.summary.root_divergences} root divergence
        {doc.summary.root_divergences === 1 ? "" : "s"}
        {doc.summary.first_divergence &&
          ` · first at t=${doc.summary.first_divergence.t} in ${doc.summary.first_divergence.component} (${doc.summary.first_divergence.mp_key})`}
        {doc.summary.cells.absorbed_by_override > 0 &&
          ` · ${doc.summary.cells.absorbed_by_override} absorbed by a tolerance override`}
      </p>

      {!doc.summary.graph_available && (
        <p className="rd-note">
          no model graph was supplied, so `root` vs `inherited` was decided from the earliest
          divergence alone (class basis {doc.findings[0]?.class_basis ?? "no_graph_earliest_t"})
        </p>
      )}
      <p className="rd-note rd-digests">
        A model {shortDigest(doc.a.model_digest)} · B model {shortDigest(doc.b.model_digest)}
      </p>
    </section>
  );
}
