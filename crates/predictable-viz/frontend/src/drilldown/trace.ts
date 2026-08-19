// The `explain()` trace tree, as the engine serialises it.
//
// These types mirror `crates/predictable-engine/src/explain.rs` field for field
// (04-verify.md §3.2). The panel renders the tree **verbatim**: it never
// recomputes a value, never reorders children, and never drops a field the
// engine chose to emit. Everything below is presentation over that JSON.

export interface TraceSpan {
  file: string;
  line: number;
  col: number;
  byte_start: number;
  byte_end: number;
}

export interface TraceModelpoint {
  key: string;
  row: number;
}

export interface TraceSource {
  file?: string;
  row?: number;
  column?: string;
  assumption_set?: string;
  line?: number;
}

/** One key of a `Lookup`, with the policy that fired. */
export interface LookupKey {
  name: string;
  requested: string;
  resolved: string;
  /** `exact` | `clamp` | `step` | `interpolate`. */
  policy: string;
  /** True when the policy changed the key — the silent-wrongness bit. */
  fired: boolean;
}

export interface AggOver {
  component: string;
  timing?: string;
  t_from: number;
  t_to: number;
  discount?: string;
  exponent_rule?: string;
}

export interface AggTerm {
  t: number;
  value: number;
  disc?: number;
  contribution: number;
  included?: boolean;
}

/** The closed node-variant set of 04-verify.md §3.2. */
export type NodeKind =
  | "Component"
  | "Binary"
  | "Unary"
  | "Ref"
  | "Lag"
  | "At"
  | "Input"
  | "Lit"
  | "If"
  | "Lookup"
  | "Call"
  | "Agg";

export interface TraceNode {
  node: NodeKind;
  /** `root.children[0].children[2]` — the join key with `notes`. */
  path: string;
  value: number;
  id?: string;
  ref?: string;
  op?: string;
  fn?: string;
  table?: string;
  kind?: string;
  t?: number;
  lag?: number;
  at?: number;
  taken?: string;
  /** `computed` | `init` | `pre_origin_default`. */
  resolution?: string;
  init_expr?: string;
  note?: string;
  text?: string;
  dtype?: string;
  unit?: string;
  timing?: string;
  shape?: string;
  stage?: number;
  expr?: string;
  span?: TraceSpan;
  modelpoint?: TraceModelpoint;
  source?: TraceSource;
  keys?: LookupKey[];
  row?: number;
  table_digest?: string;
  over?: AggOver;
  terms?: AggTerm[];
  term_count?: number;
  terms_truncated?: boolean;
  from_timing?: string;
  to_timing?: string;
  elided?: number;
  children?: TraceNode[];
}

export interface TraceNote {
  code: string;
  severity: string;
  path: string;
  message: string;
  component: string;
  t?: number;
}

export interface Trace {
  format: string;
  kind: string;
  run: string;
  root: TraceNode;
  notes?: TraceNote[];
  truncated: { depth: number; elided_nodes: number };
}

/** A trace flattened for rendering: one row per visible node, with its depth. */
export interface TraceRow {
  node: TraceNode;
  depth: number;
  /** True when the node has children that the current expansion hides. */
  collapsed: boolean;
  /** Box-drawing prefix, one glyph per ancestor level plus this node's tee. */
  prefix: string;
}

function hasChildren(node: TraceNode): boolean {
  return (node.children?.length ?? 0) > 0;
}

/**
 * Depth-first flatten in the engine's own child order. `collapsedPaths` holds
 * the paths the reader folded shut; everything else is shown.
 */
export function flattenTrace(
  root: TraceNode,
  collapsedPaths: ReadonlySet<string> = new Set(),
): TraceRow[] {
  const rows: TraceRow[] = [];
  const walk = (node: TraceNode, depth: number, prefix: string, last: boolean) => {
    const tee = depth === 0 ? "" : last ? "└─ " : "├─ ";
    const collapsed = hasChildren(node) && collapsedPaths.has(node.path);
    rows.push({ node, depth, collapsed, prefix: prefix + tee });
    if (collapsed || !hasChildren(node)) return;
    const childPrefix = depth === 0 ? "" : prefix + (last ? "   " : "│  ");
    const kids = node.children!;
    kids.forEach((child, i) => walk(child, depth + 1, childPrefix, i === kids.length - 1));
  };
  walk(root, 0, "", true);
  return rows;
}

/** Notes indexed by the node path they were raised at. */
export function notesByPath(notes: readonly TraceNote[] = []): Map<string, TraceNote[]> {
  const map = new Map<string, TraceNote[]>();
  for (const note of notes) {
    const list = map.get(note.path);
    if (list) list.push(note);
    else map.set(note.path, [note]);
  }
  return map;
}

/**
 * The line label for a node — the variant's own identity, not a summary.
 * Mirrors the closed variant set; an unknown tag falls back to the tag itself
 * rather than being dropped, so a future engine field is visible, not silent.
 */
export function nodeLabel(node: TraceNode): string {
  switch (node.node) {
    case "Component":
      return `${node.id ?? "?"}${node.t === undefined ? "" : `  t=${node.t}`}`;
    case "Binary":
      return `binary ${node.op ?? "?"}`;
    case "Unary":
      return `unary ${node.op ?? "?"}`;
    case "Ref":
      return `${node.ref ?? "?"}${node.t === undefined ? "" : `  t=${node.t}`}`;
    case "Lag":
      return `${node.ref ?? "?"}[t-${node.lag ?? 1}]${node.t === undefined ? "" : `  t=${node.t}`}`;
    case "At":
      return `${node.ref ?? "?"}@${node.at ?? 0}`;
    case "Input":
      return `${node.ref ?? "?"}  ${(node.kind ?? "input").toLowerCase()}`;
    case "Lit":
      return `literal`;
    case "If":
      return `if → ${node.taken ?? "?"}`;
    case "Lookup":
      return `${node.table ?? "?"}[${(node.keys ?? []).map((k) => k.resolved).join(", ")}]`;
    case "Call":
      return `${node.fn ?? "?"}()`;
    case "Agg":
      return `${node.ref ?? node.over?.component ?? "?"}`;
    default:
      return String(node.node);
  }
}

export type BadgeTone = "info" | "warn";

export interface Badge {
  text: string;
  tone: BadgeTone;
  title?: string;
}

/**
 * The badges a node carries: pre-origin resolution, lookup policies that fired,
 * `retime` casts, and truncation. Clamped and stepped lookups are warnings —
 * 04-verify.md §3.4 is explicit that this is where silent wrongness lives.
 */
export function nodeBadges(node: TraceNode): Badge[] {
  const badges: Badge[] = [];
  if (node.resolution === "init") {
    badges.push({
      tone: "warn",
      text: `t=${node.t} before origin → used init = ${node.init_expr ?? "declared init"}`,
    });
  } else if (node.resolution === "pre_origin_default") {
    badges.push({
      tone: "warn",
      text: `t=${node.t} before origin → no init, used ${formatValue(node.value)}`,
    });
  }
  for (const key of node.keys ?? []) {
    if (!key.fired) continue;
    badges.push({
      tone: "warn",
      text: `${key.name} ${key.policy} ${key.requested} → ${key.resolved}`,
      title: `key policy \`${key.policy}\` changed the requested key`,
    });
  }
  if (node.fn === "retime" && node.from_timing && node.to_timing) {
    badges.push({
      tone: "info",
      text: `timing ${node.from_timing} → ${node.to_timing}, value unchanged`,
    });
  }
  if (node.terms_truncated) {
    badges.push({ tone: "info", text: `terms truncated (${node.term_count ?? "?"} total)` });
  }
  if (node.elided) {
    badges.push({ tone: "info", text: `${node.elided} children elided` });
  }
  return badges;
}

/** `models/term/model.pir:148` — the clickable span label. */
export function spanLabel(span: TraceSpan | undefined): string | null {
  return span ? `${span.file}:${span.line}` : null;
}

/**
 * Display formatting: thousands separators, at most four decimals, sign never
 * suppressed. The full `f64` stays available (see {@link formatExact}) because
 * "it matched to 2 dp" is not a reconciliation (05-viz.md §5.2).
 */
export function formatValue(value: number): string {
  if (!Number.isFinite(value)) return String(value);
  const abs = Math.abs(value);
  if (abs !== 0 && (abs < 1e-4 || abs >= 1e12)) return value.toExponential(6);
  return value.toLocaleString("en-US", { maximumFractionDigits: 4, minimumFractionDigits: 0 });
}

/** The engine's `f64` verbatim — used for hover titles and for TSV copy. */
export function formatExact(value: number): string {
  return String(value);
}

/** Deltas always carry an explicit sign glyph as well as a colour. */
export function formatDelta(value: number): string {
  const sign = value > 0 ? "+" : value < 0 ? "−" : "±";
  return `${sign}${formatValue(Math.abs(value))}`;
}
