import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { InlineDataSource, readInlinePayload, selectDataSource } from "./datasource";
import { attachPackEngine, type EnginePayload } from "./wasm/engine";
import { workerEngine } from "./graph/useLayout";
import { DrilldownRoot, isDrilldownUrl } from "./drilldown/mount";
import { RunDiffRoot, isRunDiffUrl } from "./rundiff/mount";
import "./style.css";

// Mode selection (§1.1) lives in `datasource.ts`: an inline payload wins when
// one was embedded by `predictable export --format html`; otherwise we are being
// served by the local axum server and the token is in the URL fragment.
const source = selectDataSource();

// §1.5: wasm is a capability, not a mode. A governance pack exported with
// `--engine wasm` upgrades its own source in place *before* the first render,
// so no screen ever sees `capabilities()` change under it. A pack without an
// engine, and the server case, render immediately.
function upgraded(): Promise<void> {
  if (!(source instanceof InlineDataSource)) return Promise.resolve();
  const payload = readInlinePayload() as
    | (ReturnType<typeof readInlinePayload> & { engine?: EnginePayload })
    | null;
  if (!payload?.engine) return Promise.resolve();
  // A pack whose engine will not instantiate still opens: the manifest, the IR
  // and the numbers are the evidence, and the trace button greys itself out.
  return attachPackEngine(source, payload).then(
    () => undefined,
    (error: unknown) => console.error("the embedded engine did not load", error),
  );
}

void upgraded().then(() => {
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      {/* Screen 3 (T30) claims `?view=mp&mp=…`; everything else is the explorer. */}
      {isDrilldownUrl() ? (
        <DrilldownRoot source={source} />
      ) : /* Screen 2 (T31) claims `?view=diff&a=…&b=…`. */ isRunDiffUrl() ? (
        <RunDiffRoot source={source} />
      ) : (
        <App source={source} engine={workerEngine()} />
      )}
    </StrictMode>,
  );
});
