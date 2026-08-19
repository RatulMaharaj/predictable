import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { viteSingleFile } from "vite-plugin-singlefile";

// One bundle, three hosts (05-viz.md §1.1). The Rust server embeds
// `dist/index.html` with `include_str!`, the notebook path inlines it into an
// iframe `srcdoc`, and `predictable export --format html` inlines it again with
// the payload — so the build must emit exactly one self-contained file with no
// CDN and no network fetches at all.
export default defineConfig({
  plugins: [react(), viteSingleFile()],
  // The layout worker is inlined (`?worker&inline`) so the bundle stays one
  // file. It must be built as an ES module: the default `iife` worker wrapper
  // does not survive `inlineDynamicImports` in the parent chunk.
  worker: { format: "es" },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    assetsInlineLimit: 100_000_000,
    cssCodeSplit: false,
  },
  server: {
    // `npm run dev` proxies the API to a server started by `results.show()`.
    proxy: { "/api": "http://127.0.0.1:7391" },
  },
});
