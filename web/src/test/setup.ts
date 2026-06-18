// Vitest setup (#45): register the @testing-library/jest-dom matchers (e.g.
// toBeInTheDocument, toHaveTextContent) and auto-clean the rendered DOM between
// tests so component tests don't leak nodes into each other. Referenced from
// vite.config.ts `test.setupFiles`.
import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

afterEach(() => {
  cleanup();
});
