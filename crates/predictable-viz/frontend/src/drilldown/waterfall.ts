// The cashflow waterfall's geometry (05-viz.md §3.1).
//
// Bars keep the model's declaration order — authorial intent, not magnitude —
// unless the reader asks otherwise, and every bar carries its timing glyph:
// the single most common Prophet reconciliation error is a timing mismatch, and
// a chart that hides timing hides the bug.

export type Timing = "start" | "end" | "mid" | "point" | undefined;

/** One flow into the waterfall, as declared by the model. */
export interface Flow {
  component: string;
  value: number;
  timing: Timing;
  unit?: string;
  /** `Output` draws with the graph's Output node styling, and closes the chart. */
  kind?: "Input" | "Derived" | "Output";
  /** Position in the model's declaration order (IR §4.1 rule 2). */
  declarationIndex: number;
}

export interface Bar extends Flow {
  /** Running total before this bar. */
  start: number;
  /** Running total after this bar. */
  end: number;
  /** True for the closing total bar, which is drawn from zero. */
  total: boolean;
  glyph: string;
  timingLabel: string;
}

export interface WaterfallLayout {
  bars: Bar[];
  /** The running total after every flow. */
  total: number;
  /** True when at least two different timings are summed — §3.1's warning. */
  mixedTimings: boolean;
  min: number;
  max: number;
}

/**
 * Timing is a letter as well as a colour: colour is never the only channel
 * (§5.2), and these glyphs are what the mock draws beside each bar.
 */
export function timingGlyph(timing: Timing): string {
  switch (timing) {
    case "start":
      return "▙";
    case "end":
      return "▟";
    case "mid":
      return "◧";
    case "point":
      return "◆";
    default:
      return "·";
  }
}

export function timingLabel(timing: Timing): string {
  return timing ?? "untimed";
}

export interface WaterfallOptions {
  /** The "sort by magnitude" toggle; off by default, per §3.1. */
  sortByMagnitude?: boolean;
}

/**
 * Lay the flows out. Flows accumulate left to right into a running total; the
 * closing bar (the `Output` flow, when one is present) is drawn from zero so it
 * reads as the identity the graph gave it, not as one more increment.
 */
export function buildWaterfall(flows: readonly Flow[], options: WaterfallOptions = {}): WaterfallLayout {
  const closing = flows.find((f) => f.kind === "Output");
  const steps = flows.filter((f) => f !== closing);
  const ordered = [...steps].sort((a, b) =>
    options.sortByMagnitude
      ? Math.abs(b.value) - Math.abs(a.value) || a.declarationIndex - b.declarationIndex
      : a.declarationIndex - b.declarationIndex,
  );

  const bars: Bar[] = [];
  let running = 0;
  let min = 0;
  let max = 0;
  for (const flow of ordered) {
    const start = running;
    running += flow.value;
    min = Math.min(min, start, running);
    max = Math.max(max, start, running);
    bars.push({
      ...flow,
      start,
      end: running,
      total: false,
      glyph: timingGlyph(flow.timing),
      timingLabel: timingLabel(flow.timing),
    });
  }
  if (closing) {
    min = Math.min(min, 0, closing.value);
    max = Math.max(max, 0, closing.value);
    bars.push({
      ...closing,
      start: 0,
      end: closing.value,
      total: true,
      glyph: timingGlyph(closing.timing),
      timingLabel: timingLabel(closing.timing),
    });
  }

  const timings = new Set(steps.map((f) => timingLabel(f.timing)));
  return { bars, total: running, mixedTimings: timings.size > 1, min, max };
}

/** Map a value onto the plot's y pixel, given the band the bars span. */
export function yScale(waterfall: WaterfallLayout, height: number) {
  const span = waterfall.max - waterfall.min || 1;
  return (value: number) => height - ((value - waterfall.min) / span) * height;
}

/** The same waterfall as rows, for the chart's table equivalent (§5.2). */
export function waterfallRows(waterfall: WaterfallLayout): string[][] {
  return waterfall.bars.map((bar) => [
    bar.component,
    bar.timingLabel,
    String(bar.value),
    String(bar.end),
  ]);
}
