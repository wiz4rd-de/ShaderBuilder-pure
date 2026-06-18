import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn(
  (_cmd: string, _args?: Record<string, unknown>): Promise<unknown> => Promise.resolve(),
);
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => invoke(cmd, args),
}));

import { useDocumentStore } from "../store/documentStore";
import { resetIdsForTest } from "../store/ids";
import { ParameterPanel } from "./ParameterPanel";

function store() {
  return useDocumentStore.getState();
}

/** The args of the most recent set_parameter call, or null. */
function lastSetParameter(): Record<string, unknown> | null {
  for (let i = invoke.mock.calls.length - 1; i >= 0; i--) {
    const [cmd, args] = invoke.mock.calls[i]!;
    if (cmd === "set_parameter") {
      return args ?? null;
    }
  }
  return null;
}

/** Add a Param node declaring `name` to the active graph pass. */
function addParam(name: string, over: Record<string, unknown> = {}): void {
  act(() => {
    store().addNode(
      "param",
      { x: 0, y: 0 },
      { name, label: name, default: 0.5, min: 0, max: 1, step: 0.01, ...over },
    );
  });
}

beforeEach(() => {
  resetIdsForTest();
  store().reset();
  invoke.mockClear();
  // Drive the rAF-coalescing sender synchronously in jsdom.
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
    cb(0);
    return 1;
  });
  vi.stubGlobal("cancelAnimationFrame", () => {});
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("ParameterPanel", () => {
  it("renders a placeholder when no parameters are declared", () => {
    render(<ParameterPanel />);
    expect(screen.getByText(/No parameters declared/i)).toBeTruthy();
  });

  it("renders a slider for a declared Param node, bounded by its range/step", async () => {
    addParam("gamma", { label: "Gamma", min: 0, max: 3, step: 0.05 });
    render(<ParameterPanel />);
    const slider = (await screen.findByLabelText("gamma slider")) as HTMLInputElement;
    expect(slider.type).toBe("range");
    expect(slider.min).toBe("0");
    expect(slider.max).toBe("3");
    expect(slider.step).toBe("0.05");
  });

  it("moving a slider fires set_parameter (no recompile) with the clamped value", async () => {
    addParam("bright", { min: 0, max: 1 });
    render(<ParameterPanel />);
    const slider = await screen.findByLabelText("bright slider");
    fireEvent.change(slider, { target: { value: "0.8" } });
    await waitFor(() => expect(lastSetParameter()).toEqual({ name: "bright", value: 0.8 }));
    // No recompile command was issued.
    expect(invoke.mock.calls.some(([c]) => c === "compile_graph")).toBe(false);
  });

  it("reset restores the declared default and the button disables at default", async () => {
    addParam("sat", { default: 0.5, min: 0, max: 1 });
    render(<ParameterPanel />);
    const slider = await screen.findByLabelText("sat slider");
    fireEvent.change(slider, { target: { value: "0.9" } });
    expect((slider as HTMLInputElement).value).toBe("0.9");
    const reset = screen.getByLabelText("Reset sat to default");
    fireEvent.click(reset);
    expect((slider as HTMLInputElement).value).toBe("0.5");
    expect((reset as HTMLButtonElement).disabled).toBe(true);
    expect(lastSetParameter()).toEqual({ name: "sat", value: 0.5 });
  });

  it("reconciles the slider set on graph edits, preserving unrelated values", async () => {
    addParam("a", { default: 0 });
    addParam("b", { default: 0 });
    render(<ParameterPanel />);
    const sliderA = await screen.findByLabelText("a slider");
    fireEvent.change(sliderA, { target: { value: "0.7" } });
    expect((sliderA as HTMLInputElement).value).toBe("0.7");

    // Add a third param `c` — `a`'s value must survive.
    addParam("c", { default: 0.2 });
    const sliderC = await screen.findByLabelText("c slider");
    expect((sliderC as HTMLInputElement).value).toBe("0.2");
    expect((screen.getByLabelText("a slider") as HTMLInputElement).value).toBe("0.7");
  });
});
