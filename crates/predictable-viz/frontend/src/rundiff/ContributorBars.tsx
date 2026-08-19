import { cohortSentence, type CohortResult } from "./cohort";
import type { ContributorRanking } from "./contributors";
import { shortName } from "./attribution";

const SHOWN = 12;

/**
 * Contributor bars and the cohort sentence (05-viz.md §4.2, right).
 *
 * Both are derived in the client from the two runs' series, so both are
 * labelled: the bars carry their method string and the cohort is presented as a
 * hypothesis, per §0's rule on chart-only derived quantities.
 */
export function ContributorBars({
  ranking,
  cohort,
  onOpen,
}: {
  ranking: ContributorRanking;
  cohort: CohortResult | null;
  onOpen?: (mp: string) => void;
}) {
  const shown = ranking.contributors.filter((c) => c.cells > 0).slice(0, SHOWN);
  const rest = ranking.differing.length - shown.length;
  const max = Math.max(...shown.map((c) => c.absDelta), Number.MIN_VALUE);

  return (
    <section className="rd-contributors" aria-label="contributors">
      <h2>
        contributors to <code>{shortName(ranking.component)}</code> movement
      </h2>

      {shown.length === 0 && <p className="rd-note">no modelpoint moved outside tolerance</p>}

      <ol className="rd-bars">
        {shown.map((c) => (
          <li key={c.mp} className="rd-bar-row" data-mp={c.mp}>
            <button type="button" className="rd-mp" onClick={() => onOpen?.(c.mp)}>
              {c.mp}
            </button>
            <span className={c.delta >= 0 ? "rd-amount rd-pos" : "rd-amount rd-neg"}>
              {c.delta >= 0 ? "+" : "−"}
              {Math.abs(c.delta).toLocaleString("en-US", { maximumFractionDigits: 2 })}
            </span>
            <span className="rd-share">{(c.share * 100).toFixed(1)}%</span>
            <span
              className="rd-bar"
              style={{ width: `${(c.absDelta / max) * 100}%` }}
              aria-hidden="true"
            />
            <span className="rd-first-t">from t={c.tFirst}</span>
          </li>
        ))}
      </ol>

      {rest > 0 && (
        <p className="rd-more" data-testid="more">
          … {rest} more differing modelpoints
        </p>
      )}

      <p className="rd-method" data-testid="contributor-method">
        derived in the browser · {ranking.method}
      </p>

      {cohort && (
        <p className="rd-cohort" data-testid="cohort" data-kind={cohort.kind}>
          <span className="rd-hypothesis-tag">hypothesis</span> {cohortSentence(cohort)}
          {cohort.kind === "cohort" && (
            <span className="rd-cohort-quality">
              {" "}
              · precision {(cohort.precision * 100).toFixed(0)}%, recall{" "}
              {(cohort.recall * 100).toFixed(0)}%
            </span>
          )}
        </p>
      )}
      {cohort && (
        <p className="rd-method">derived in the browser · {cohort.method}</p>
      )}
    </section>
  );
}
