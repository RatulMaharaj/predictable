// A JS implementation of the `predictable-wasm` ABI, for tests.
//
// The point is not to fake the engine's *answers* — those are checked in Rust,
// against `models/term_annual/expected/` and against wasmtime. The point is to
// exercise the marshalling: the length-prefixed buffer, the allocator, the
// free, and the memory growth that detaches every existing view. A test that
// used a `.wasm` file would need the build to have run and would still not
// cover the growth case on demand.

import type { EngineExports } from "./engine";

/** How the fake answers. Return the response object; it is JSON-encoded here. */
export type Handler = (request: Record<string, unknown>) => unknown;

export interface FakeEngine extends EngineExports {
  /** Every request the page made, decoded — the assertion surface. */
  readonly requests: Record<string, unknown>[];
  /** Buffers handed out and not yet freed. Must be empty when a test ends. */
  outstanding(): number;
}

/**
 * A fake with a real `WebAssembly.Memory`, so the detach-on-grow hazard is
 * real: `grow` in `pv_alloc` replaces `memory.buffer`, and any caller that
 * cached a view over the old one fails here exactly as it would in a browser.
 */
export function fakeEngine(handler: Handler, options: { growAfter?: number } = {}): FakeEngine {
  const memory = new WebAssembly.Memory({ initial: 2, maximum: 64 });
  const requests: Record<string, unknown>[] = [];
  const live = new Map<number, number>();
  // Leave the first page alone so pointer 0 is never a valid allocation.
  let next = 65_536;
  let allocations = 0;

  const alloc = (len: number): number => {
    allocations += 1;
    if (options.growAfter !== undefined && allocations > options.growAfter) {
      memory.grow(1);
    }
    while (next + len > memory.buffer.byteLength) memory.grow(1);
    const ptr = next;
    next += Math.max(len, 1);
    // 8-byte alignment keeps the `DataView` reads honest.
    next = (next + 7) & ~7;
    live.set(ptr, len);
    return ptr;
  };

  return {
    memory,
    requests,
    outstanding: () => live.size,
    pv_alloc: alloc,
    pv_free(ptr: number) {
      live.delete(ptr);
    },
    pv_call(ptr: number, len: number): number {
      const text = new TextDecoder().decode(new Uint8Array(memory.buffer, ptr, len));
      live.delete(ptr);
      const request = JSON.parse(text) as Record<string, unknown>;
      requests.push(request);
      const body = new TextEncoder().encode(JSON.stringify(handler(request)));
      const out = alloc(body.length + 4);
      new DataView(memory.buffer).setUint32(out, body.length, true);
      new Uint8Array(memory.buffer, out + 4, body.length).set(body);
      return out;
    },
  };
}
