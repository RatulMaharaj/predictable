#!/usr/bin/env bash
# Build the SPA into `dist/index.html`, the single file `src/assets.rs` embeds.
#
# `dist/index.html` is checked in on purpose: `cargo build` must work on a
# machine with no Node installed, and the Rust tests must not depend on npm. Run
# this when the frontend changes and commit the result.
set -euo pipefail
cd "$(dirname "$0")"

npm ci --no-audit --no-fund
npm run build

test -f dist/index.html || { echo "build produced no dist/index.html" >&2; exit 1; }
# vite-plugin-singlefile inlines everything; anything left in dist/assets would
# be fetched at runtime, which a governance pack (§1.4) must never do.
if [ -d dist/assets ] && [ -n "$(ls -A dist/assets 2>/dev/null)" ]; then
  echo "dist/assets is not empty: the bundle is not self-contained" >&2
  exit 1
fi
# §1.6's JS budget: < 900 kB brotli without wasm. Brotli is not on every
# machine, so the gate is gzip at the same figure — gzip is never smaller than
# brotli, so passing here passes there.
bytes=$(gzip -9 -c dist/index.html | wc -c | tr -d ' ')
budget=921600
if [ "$bytes" -gt "$budget" ]; then
  echo "the bundle is $bytes bytes gzipped, over §1.6's $budget byte budget" >&2
  exit 1
fi
echo "built $(wc -c < dist/index.html) bytes into dist/index.html ($bytes gzipped)"
