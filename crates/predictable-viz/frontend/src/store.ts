// View state, and its encoding into the URL — 05-viz.md §5.1.
//
// > Every view is fully described by its URL … Pasting a URL into a review
// > comment reproduces the exact screen. No exceptions, including transient
// > panel state.
//
// The token lives in the fragment (§1.2), so view state goes in the *query*
// string; a static export reads the same keys out of `location.hash` after the
// token. `encodeView`/`decodeView` are pure and round-trip, which is the property
// the test asserts.

import { create } from "zustand";
import { NO_FILTERS, type Filters } from "./graph/model";

export interface View {
  selected?: string;
  focus: boolean;
  impact: boolean;
  layers: boolean;
  lints: boolean;
  filters: Filters;
  /** Free-text query left over from the palette, so a search is linkable. */
  query: string;
  /** Modelpoint and `t` the inspector's trace button will use (§2.4). */
  modelpoint?: string;
  t?: number;
}

export const INITIAL_VIEW: View = {
  focus: false,
  impact: false,
  layers: false,
  lints: true,
  filters: NO_FILTERS,
  query: "",
};

const LIST_KEYS = ["module", "unit", "kind", "tag"] as const;

export function encodeView(view: View): string {
  const params = new URLSearchParams();
  if (view.selected) params.set("sel", view.selected);
  if (view.focus) params.set("focus", "1");
  if (view.impact) params.set("impact", "1");
  if (view.layers) params.set("layers", "1");
  if (!view.lints) params.set("lints", "0");
  if (view.query) params.set("q", view.query);
  if (view.filters.modules.length) params.set("module", view.filters.modules.join(","));
  if (view.filters.units.length) params.set("unit", view.filters.units.join(","));
  if (view.filters.kinds.length) params.set("kind", view.filters.kinds.join(","));
  if (view.filters.tags.length) params.set("tag", view.filters.tags.join(","));
  if (view.filters.affects) params.set("affects", view.filters.affects);
  if (view.modelpoint) params.set("mp", view.modelpoint);
  if (view.t !== undefined) params.set("t", String(view.t));
  return params.toString();
}

export function decodeView(query: string): View {
  const params = new URLSearchParams(query.startsWith("?") ? query.slice(1) : query);
  const list = (key: (typeof LIST_KEYS)[number]) => {
    const raw = params.get(key);
    return raw ? raw.split(",").filter((s) => s.length > 0) : [];
  };
  const t = params.get("t");
  return {
    selected: params.get("sel") ?? undefined,
    focus: params.get("focus") === "1",
    impact: params.get("impact") === "1",
    layers: params.get("layers") === "1",
    lints: params.get("lints") !== "0",
    query: params.get("q") ?? "",
    filters: {
      modules: list("module"),
      units: list("unit"),
      kinds: list("kind"),
      tags: list("tag"),
      affects: params.get("affects") ?? undefined,
    },
    modelpoint: params.get("mp") ?? undefined,
    t: t === null || t === "" ? undefined : Number(t),
  };
}

export interface ViewStore extends View {
  set(patch: Partial<View>): void;
  select(id: string | undefined): void;
  toggle(key: "focus" | "impact" | "layers" | "lints"): void;
  toggleFacet(facet: keyof Omit<Filters, "affects">, value: string): void;
  reset(): void;
}

/**
 * Write the view into `history` without a navigation. Exported so tests can
 * drive it with a fake history, and so the App can call it from one place.
 */
export function pushView(view: View, history: History = globalThis.history): void {
  const query = encodeView(view);
  const url = `${location.pathname}${query ? `?${query}` : ""}${location.hash}`;
  history.replaceState(null, "", url);
}

export const useView = create<ViewStore>((set, get) => ({
  ...INITIAL_VIEW,
  ...(typeof location === "undefined" ? {} : decodeView(location.search)),
  set(patch) {
    set(patch);
    if (typeof location !== "undefined") pushView({ ...get(), ...patch });
  },
  select(id) {
    get().set({ selected: id });
  },
  toggle(key) {
    get().set({ [key]: !get()[key] } as Partial<View>);
  },
  toggleFacet(facet, value) {
    const current = get().filters[facet];
    const next = current.includes(value)
      ? current.filter((v) => v !== value)
      : [...current, value];
    get().set({ filters: { ...get().filters, [facet]: next } });
  },
  reset() {
    get().set({ ...INITIAL_VIEW });
  },
}));
