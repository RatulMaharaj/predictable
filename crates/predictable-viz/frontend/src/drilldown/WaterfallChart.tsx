import { buildWaterfall, yScale, type Flow } from "./waterfall";
import { formatDelta, formatExact, formatValue } from "./trace";

export interface WaterfallProps {
  flows: readonly Flow[];
  t: number;
  sortByMagnitude?: boolean;
  onToggleSort?: () => void;
  /** Click a bar → `explain()` for that component at that `t` (§3.1). */
  onSelect?: (component: string) => void;
  /** The component the trace panel is hovering, drawn highlighted. */
  highlight?: string | null;
  /** The table equivalent behind a toggle (§5.2) — accessibility and audit. */
  showTable?: boolean;
  width?: number;
  height?: number;
}

/**
 * Hand-drawn SVG (§3.1). Bars are in declaration order unless the reader asks
 * for magnitude; each carries its timing glyph, the connector line carries the
 * running total, and the closing Output bar is drawn from zero.
 */
export function Waterfall({
  flows,
  t,
  sortByMagnitude = false,
  onToggleSort,
  onSelect,
  highlight,
  showTable = false,
  width = 420,
  height = 220,
}: WaterfallProps) {
  const waterfall = buildWaterfall(flows, { sortByMagnitude });
  const barWidth = waterfall.bars.length ? width / waterfall.bars.length : width;
  const y = yScale(waterfall, height);
  const zero = y(0);

  return (
    <section className="waterfall" aria-label={`cashflow waterfall at t = ${t}`}>
      <header>
        <h2>waterfall at t = {t}</h2>
        <label>
          <input type="checkbox" checked={sortByMagnitude} onChange={onToggleSort} />
          sort by magnitude
        </label>
      </header>

      <svg
        className="waterfall-svg"
        viewBox={`0 0 ${width} ${height}`}
        width="100%"
        height={height}
        role="img"
        aria-label={`waterfall of ${waterfall.bars.length} components, total ${formatValue(waterfall.total)}`}
      >
        <line className="wf-axis" x1={0} y1={zero} x2={width} y2={zero} />
        {waterfall.bars.map((bar, i) => {
          const x = i * barWidth;
          const top = Math.min(y(bar.start), y(bar.end));
          const size = Math.max(1, Math.abs(y(bar.end) - y(bar.start)));
          const classes = [
            "wf-bar",
            bar.value < 0 ? "is-negative" : "is-positive",
            bar.total ? "is-total" : "",
            bar.kind === "Output" ? "is-output" : "",
            highlight && highlight.endsWith(bar.component) ? "is-highlight" : "",
          ]
            .filter(Boolean)
            .join(" ");
          return (
            <g key={bar.component} className={classes} data-component={bar.component}>
              {i > 0 && !bar.total ? (
                <line
                  className="wf-connector"
                  x1={x - barWidth * 0.1}
                  y1={y(bar.start)}
                  x2={x + barWidth * 0.1}
                  y2={y(bar.start)}
                />
              ) : null}
              <rect
                x={x + barWidth * 0.15}
                y={top}
                width={barWidth * 0.7}
                height={size}
                tabIndex={0}
                role="button"
                aria-label={`${bar.component} ${formatDelta(bar.value)} ${bar.timingLabel}`}
                onClick={() => onSelect?.(bar.component)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") onSelect?.(bar.component);
                }}
              >
                <title>
                  {bar.component} {formatExact(bar.value)} · timing {bar.timingLabel} · running{" "}
                  {formatExact(bar.end)}
                </title>
              </rect>
              <text className="wf-glyph" x={x + barWidth * 0.5} y={height - 14}>
                {bar.glyph}
              </text>
              <text className="wf-timing" x={x + barWidth * 0.5} y={height - 3}>
                {bar.timingLabel}
              </text>
            </g>
          );
        })}
      </svg>

      <p className="wf-total">
        Δ total <span title={formatExact(waterfall.total)}>{formatDelta(waterfall.total)}</span>
      </p>
      {waterfall.mixedTimings ? (
        <p className="wf-warn" role="note">
          ⚠ flows with different timings are summed here; timings are shown per bar.
        </p>
      ) : null}

      {showTable ? (
        <table className="wf-table">
          <thead>
            <tr>
              <th>component</th>
              <th>timing</th>
              <th>value</th>
              <th>running</th>
            </tr>
          </thead>
          <tbody>
            {waterfall.bars.map((bar) => (
              <tr key={bar.component}>
                <td>{bar.component}</td>
                <td>{bar.timingLabel}</td>
                <td>{formatExact(bar.value)}</td>
                <td>{formatExact(bar.end)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : null}
    </section>
  );
}
