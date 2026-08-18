import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Tests run in jsdom. The layout worker is never started in a test: `useLayout`
// takes its `LayoutEngine` by injection, and the ELK-facing half is tested
// directly against `elkjs` in `layout.test.ts` (§5.4).
export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
