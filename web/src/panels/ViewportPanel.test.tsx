import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn(
  (_cmd: string, _args?: Record<string, unknown>): Promise<unknown> => Promise.resolve(),
);
vi.mock("@tauri-apps/api/core", () => ({ invoke: (cmd: string, args?: Record<string, unknown>) => invoke(cmd, args) }));

import { ViewportPanel } from "./ViewportPanel";

/** The args of the most recent set_simulated_viewport call. */
function lastViewportArgs(): Record<string, unknown> | null {
  for (let i = invoke.mock.calls.length - 1; i >= 0; i--) {
    const [cmd, args] = invoke.mock.calls[i]!;
    if (cmd === "set_simulated_viewport") {
      return args ?? null;
    }
  }
  return null;
}

beforeEach(() => {
  invoke.mockClear();
});

describe("ViewportPanel", () => {
  it("calls set_simulated_viewport with the exact Rust signature on enable", () => {
    render(<ViewportPanel />);
    fireEvent.click(screen.getByLabelText("Simulated viewport enabled"));
    const args = lastViewportArgs();
    expect(args).toEqual({
      enabled: true,
      width: 640,
      height: 480,
      integerScale: false,
    });
  });

  it("enabled=false clears the simulated viewport", () => {
    render(<ViewportPanel />);
    fireEvent.click(screen.getByLabelText("Simulated viewport enabled")); // on
    fireEvent.click(screen.getByLabelText("Simulated viewport enabled")); // off
    expect(lastViewportArgs()).toMatchObject({ enabled: false });
  });

  it("derives height from the aspect ratio and pushes it when enabled", () => {
    render(<ViewportPanel />);
    fireEvent.click(screen.getByLabelText("Simulated viewport enabled"));
    fireEvent.change(screen.getByLabelText("Viewport width"), { target: { value: "800" } });
    // 4:3 default => height 600.
    expect(lastViewportArgs()).toMatchObject({ width: 800, height: 600 });
  });

  it("pushes the integer-scale flag", () => {
    render(<ViewportPanel />);
    fireEvent.click(screen.getByLabelText("Simulated viewport enabled"));
    fireEvent.click(screen.getByLabelText("Integer scale"));
    expect(lastViewportArgs()).toMatchObject({ integerScale: true });
  });

  it("does not call the engine while disabled (other than enable/disable)", () => {
    render(<ViewportPanel />);
    fireEvent.change(screen.getByLabelText("Viewport width"), { target: { value: "320" } });
    expect(invoke).not.toHaveBeenCalled();
  });
});
