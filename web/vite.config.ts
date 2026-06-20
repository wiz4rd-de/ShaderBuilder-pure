/// <reference types="vitest/config" />
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// Vite config tuned for Tauri (https://v2.tauri.app/start/frontend/vite/):
// a fixed dev-server port Tauri's `devUrl` points at, and no terminal clearing
// so Rust and Vite logs interleave cleanly. The `test` block configures the
// vitest component-test runner (#45): a jsdom DOM, global expect/describe, and
// a setup file that wires the @testing-library/jest-dom matchers.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    // The frontend dev server has no business watching the Rust workspace.
    watch: {
      ignored: ["**/crates/**", "**/target/**"],
    },
  },
  build: {
    // dist/ sits next to this file (web/dist) and is referenced by
    // crates/app/tauri.conf.json -> build.frontendDist.
    target: "esnext",
    outDir: "dist",
    emptyOutDir: true,
    sourcemap: true,
  },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
    css: false,
    // Component + store tests live next to their subjects as *.test.ts(x).
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
