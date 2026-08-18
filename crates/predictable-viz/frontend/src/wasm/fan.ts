// The sensitivity fan — 05-viz.md §3.3, over 01-ir.md Q12's lineage fields.
//
// A fan is N runs that share a parent. Q12 exists because inferring that from
// filenames is not acceptable for something a regulator reads, so every
// scenario the engine computes comes back carrying
// `{parent_run, parent_manifest_digest, group_id, label, varied[]}`, and this
// module reads those fields rather than re-deriving them from the request it
// just sent. If the engine and the request disagree about what was varied, the
// engine is right and the chart says so.
//
// Two rules from §3.3 are structural here, not stylistic:
//
//   · **No interpolation between computed scenarios, ever.** `FanSeries.points`
//     holds only computed `(t, value)` pairs, and every one is a marker. There
//     is no resampling step to add later, because there is nowhere to put it.
//   · **Each band is a real run and is inspectable.** A band keeps its whole
//     lineage, which is what lets the UI hand it to the diff screen as "run B".

import { ApiError } from "../datasource";
import { callEngine, type EngineExports, type EngineInputs, type SeriesColumns } from "./engine";

/** Q12's `varied[]` entry: re-derivable by diffing two manifests. */
export interface Varied {
  path: string;
  from: number;
  to: number;
}

/** Q12's `lineage`. `parent_run` is null for a base run. */
export interface Lineage {
  parent_run: string | null;
  parent_manifest_digest: string | null;
  group_id: string;
  label: string;
  varied: Varied[];
}

/** One computed point. There are never any others. */
export interface FanPoint {
  t: number;
  value: number;
}

/** One band: a real run, its lineage, and the points it computed. */
export interface FanSeries {
  label: string;
  lineage: Lineage | null;
  points: FanPoint[];
}

/** What the chart draws: a solid base line and N bands, all computed. */
export interface Fan {
  groupId: string;
  component: string;
  modelpoint: string | null;
  base: FanSeries;
  bands: FanSeries[];
  /** Always false. Present so the property is stated in the data, not the docs. */
  interpolated: false;
}

/** A scenario to compute: a label and the assumption values it sets. */
export interface ScenarioRequest {
  label: string;
  set: Record<string, number>;
}

/**
 * `base × factors` for one assumption — the shape a slider produces.
 *
 * The base value is included only when a factor of exactly 1 is asked for: the
 * fan's base line is the *unvaried* run, and duplicating it as a band would
 * draw it twice and rank it in the tornado against itself.
 */
export function scaleScenarios(
  assumption: string,
  base: number,
  factors: number[],
): ScenarioRequest[] {
  return factors
    .filter((factor) => factor !== 1)
    .map((factor) => ({
      label: `${assumption} × ${factor}`,
      set: { [assumption]: base * factor },
    }));
}

/** One long-form column set → the points for one modelpoint (or the first). */
function pointsOf(series: SeriesColumns, component: string, modelpoint: string | null): FanPoint[] {
  const column = series.columns[component];
  if (!column) {
    throw new ApiError(404, "unknown_component", `\`${component}\` is not an output of this model`);
  }
  const wanted = modelpoint ?? series.mp[0];
  const points: FanPoint[] = [];
  for (let i = 0; i < column.length; i += 1) {
    if (series.mp[i] !== wanted) continue;
    const value = column[i];
    // A cell the run did not produce is absent, not zero: drawing a gap is the
    // honest rendering and joining across it would invent a point.
    if (!Number.isFinite(value)) continue;
    points.push({ t: series.t[i], value });
  }
  return points;
}

/** The engine's `sensitivity` response, as it comes over the ABI. */
interface SensitivityResponse {
  ok: boolean;
  group_id: string;
  base: SeriesColumns;
  scenarios: { lineage: Lineage; series: SeriesColumns }[];
  interpolated: boolean;
}

/**
 * Run a fan in the page (§1.5's second unlock: no round trip, so the slider is
 * continuous rather than request-per-drag).
 */
export function runFan(
  exports: EngineExports,
  inputs: EngineInputs,
  options: {
    component: string;
    scenarios: ScenarioRequest[];
    modelpoints?: string[];
    groupId?: string;
    parentRun?: string | null;
    parentManifestDigest?: string | null;
  },
): Fan {
  const response = callEngine(exports, {
    op: "sensitivity",
    inputs,
    components: [options.component],
    modelpoints: options.modelpoints ?? [],
    scenarios: options.scenarios,
    group_id: options.groupId ?? "fan",
    parent_run: options.parentRun ?? null,
    parent_manifest_digest: options.parentManifestDigest ?? null,
  }) as SensitivityResponse & { kind?: string; error?: string };

  if (!response.ok) {
    throw new ApiError(500, response.kind ?? "engine", response.error ?? "the fan failed");
  }
  return toFan(response, options.component, options.modelpoints?.[0] ?? null);
}

/** The response → the chart's model. Separated so it is testable on its own. */
export function toFan(
  response: SensitivityResponse,
  component: string,
  modelpoint: string | null,
): Fan {
  return {
    groupId: response.group_id,
    component,
    modelpoint,
    base: {
      label: "base",
      lineage: null,
      points: pointsOf(response.base, component, modelpoint),
    },
    bands: response.scenarios.map((scenario) => ({
      label: scenario.lineage.label,
      lineage: scenario.lineage,
      points: pointsOf(scenario.series, component, modelpoint),
    })),
    interpolated: false,
  };
}

/** One bar of the tornado: Δ(output) at a single `t`, per varied assumption. */
export interface TornadoBar {
  label: string;
  /** `assumptions.x 1 → 1.1`, from the lineage, never re-derived. */
  varied: Varied[];
  base: number;
  value: number;
  delta: number;
  lineage: Lineage;
}

/**
 * The tornado view of the same data at one `t` (§3.3: "the view management
 * actually asks for"), sorted by absolute impact.
 *
 * A band with no computed point at `t` is **omitted**, not zeroed: a missing
 * bar is a scenario that did not reach that period, and a zero-length bar would
 * claim it was insensitive.
 */
export function tornadoAt(fan: Fan, t: number): TornadoBar[] {
  const base = fan.base.points.find((p) => p.t === t);
  if (!base) return [];
  const bars: TornadoBar[] = [];
  for (const band of fan.bands) {
    const point = band.points.find((p) => p.t === t);
    if (!point || !band.lineage) continue;
    bars.push({
      label: band.label,
      varied: band.lineage.varied,
      base: base.value,
      value: point.value,
      delta: point.value - base.value,
      lineage: band.lineage,
    });
  }
  // Ties break on the label so the order is stable between renders — a bar that
  // jumps places when nothing changed reads as a change.
  bars.sort((a, b) => Math.abs(b.delta) - Math.abs(a.delta) || a.label.localeCompare(b.label));
  return bars;
}

/** The y-extent of every computed point, for a shared axis across bands. */
export function fanExtent(fan: Fan): [number, number] {
  const values = [fan.base, ...fan.bands].flatMap((s) => s.points.map((p) => p.value));
  if (!values.length) return [0, 1];
  return [Math.min(...values), Math.max(...values)];
}
