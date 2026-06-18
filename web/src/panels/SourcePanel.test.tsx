import { fireEvent, render, screen, act, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn(
  (_cmd: string, _args?: Record<string, unknown>): Promise<unknown> => Promise.resolve(),
);
vi.mock("@tauri-apps/api/core", () => ({ invoke: (cmd: string, args?: Record<string, unknown>) => invoke(cmd, args) }));

// Capture the source-position listener so the test can emit events into it.
let positionHandler: ((e: { payload: unknown }) => void) | null = null;
const unlisten = vi.fn();
vi.mock("@tauri-apps/api/event", () => ({
  listen: (_event: string, handler: (e: { payload: unknown }) => void) => {
    positionHandler = handler;
    return Promise.resolve(unlisten);
  },
}));

import { SourcePanel } from "./SourcePanel";

function callsTo(cmd: string): Record<string, unknown>[] {
  return invoke.mock.calls
    .filter(([c]) => c === cmd)
    .map(([, args]) => args ?? {});
}

beforeEach(() => {
  invoke.mockClear();
  unlisten.mockClear();
  positionHandler = null;
});

describe("SourcePanel — source selection", () => {
  it("loads a test pattern with no path", () => {
    render(<SourcePanel />);
    fireEvent.change(screen.getByLabelText("Test pattern"), {
      target: { value: "checkerboard" },
    });
    // The first "Load" button is the test-pattern one.
    fireEvent.click(screen.getAllByText("Load")[0]!);
    expect(callsTo("load_test_pattern")).toContainEqual({ pattern: "checkerboard" });
  });

  it("loads an image path (empty => null => built-in)", () => {
    render(<SourcePanel />);
    fireEvent.change(screen.getByLabelText("Image path"), {
      target: { value: "/img.png" },
    });
    const loadButtons = screen.getAllByText("Load");
    fireEvent.click(loadButtons[1]!); // image Load
    expect(callsTo("load_source")).toContainEqual({ sourcePath: "/img.png" });
  });

  it("loads a PNG-sequence directory", () => {
    render(<SourcePanel />);
    fireEvent.change(screen.getByLabelText("Sequence directory"), {
      target: { value: "/frames/" },
    });
    const loadButtons = screen.getAllByText("Load");
    fireEvent.click(loadButtons[2]!); // sequence Load
    expect(callsTo("load_source_sequence")).toContainEqual({ dir: "/frames/" });
  });
});

describe("SourcePanel — transport", () => {
  it("play/pause/step/seek/set_fps map to the right commands", () => {
    render(<SourcePanel />);
    fireEvent.click(screen.getByText("Play"));
    expect(callsTo("play")).toHaveLength(1);
    fireEvent.click(screen.getByText("Pause"));
    expect(callsTo("pause")).toHaveLength(1);
    fireEvent.click(screen.getByText("Step"));
    expect(callsTo("step")).toHaveLength(1);
    fireEvent.change(screen.getByLabelText("FPS"), { target: { value: "30" } });
    expect(callsTo("set_fps")).toContainEqual({ fps: 30 });
  });

  it("syncs the frame counter from the source-position event", async () => {
    render(<SourcePanel />);
    await waitFor(() => expect(positionHandler).not.toBeNull());
    act(() => positionHandler!({ payload: { index: 5, len: 10 } }));
    expect(screen.getByLabelText("Frame counter")).toHaveTextContent("5 / 9");
  });

  it("seek is enabled once a sequence is loaded and emits seek({index})", async () => {
    render(<SourcePanel />);
    await waitFor(() => expect(positionHandler).not.toBeNull());
    act(() => positionHandler!({ payload: { index: 0, len: 12 } }));
    const slider = screen.getByLabelText("Seek") as HTMLInputElement;
    expect(slider.disabled).toBe(false);
    fireEvent.change(slider, { target: { value: "7" } });
    expect(callsTo("seek")).toContainEqual({ index: 7 });
  });

  it("unsubscribes from the source-position event on unmount", async () => {
    const { unmount } = render(<SourcePanel />);
    await waitFor(() => expect(positionHandler).not.toBeNull());
    unmount();
    await waitFor(() => expect(unlisten).toHaveBeenCalled());
  });
});
