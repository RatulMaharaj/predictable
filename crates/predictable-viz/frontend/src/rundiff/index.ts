// Screen 2 — Run Diff (05-viz.md §4.2).
//
// The screen is written against the frozen `DataSource` contract only: it
// receives a source, never constructs one, and never branches on which
// implementation it got — so the same screen serves the local server, an
// exported governance pack, and a wasm-backed pack.

export { RunDiffScreen } from "./RunDiffScreen";
export { ManifestBanner } from "./ManifestBanner";
export { AttributionPanel } from "./AttributionPanel";
export { ContributorBars } from "./ContributorBars";
export { SideBySide } from "./SideBySide";
export { RunDiffRoot, isRunDiffUrl, parseRunDiffRoute, runDiffUrl } from "./mount";
export * from "./reconcile";
export * from "./attribution";
export * from "./contributors";
export * from "./cohort";
export * from "./align";
