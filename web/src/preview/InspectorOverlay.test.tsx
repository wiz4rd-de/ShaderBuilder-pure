import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";

import type { PixelSample } from "../bindings/PixelSample";
import { InspectorOverlay } from "./InspectorOverlay";
import type { CanvasGeometry } from "./InspectorOverlay";
import { useInspectorStore } from "./inspectorStore";

const GEOMETRY: CanvasGeometry = {
  boxWidth: 512,
  boxHeight: 384,
  canvasWidth: 512,
  canvasHeight: 384,
};

function sample(rgba: [number, number, number, number]): PixelSample {
  return {
    inside: true,
    x: 128,
    y: 96,
    viewportWidth: 256,
    viewportHeight: 224,
    rgba,
    format: "rgba8Unorm",
  };
}

beforeEach(() => {
  useInspectorStore.setState({
    enabled: false,
    display: { bytes: true, srgb: false },
    hover: null,
    pinned: [],
  });
});

describe("InspectorOverlay", () => {
  it("renders nothing when disabled and there are no pins", () => {
    render(<InspectorOverlay geometry={GEOMETRY} />);
    expect(screen.queryByTestId("inspect-overlay")).toBeNull();
  });

  it("shows the hover readout with the viewport coordinate + RGBA when enabled", () => {
    useInspectorStore.setState({ enabled: true });
    useInspectorStore.getState().setHover({
      pane: { x: 256, y: 192 },
      sample: sample([1.0, 0.5, 0.0, 1.0]),
    });
    render(<InspectorOverlay geometry={GEOMETRY} />);

    const readout = screen.getByTestId("inspect-hover-readout");
    // The coordinate is the SIMULATED-VIEWPORT coordinate from the sample.
    expect(readout).toHaveTextContent("(128, 96)");
    // 0-255 bytes by default: R=255, G=128, B=0, A=255.
    expect(readout).toHaveTextContent("255, 128, 0, 255");
  });

  it("the display toggle changes the shown value, not the stored readback", () => {
    useInspectorStore.setState({ enabled: true });
    const s = sample([1.0, 0.5, 0.0, 1.0]);
    useInspectorStore.getState().setHover({ pane: { x: 0, y: 0 }, sample: s });
    const { rerender } = render(<InspectorOverlay geometry={GEOMETRY} />);
    expect(screen.getByTestId("inspect-hover-readout")).toHaveTextContent("255, 128, 0, 255");

    // Flip to 0-1 floats — the displayed text changes...
    useInspectorStore.getState().setBytes(false);
    rerender(<InspectorOverlay geometry={GEOMETRY} />);
    expect(screen.getByTestId("inspect-hover-readout")).toHaveTextContent("1, 0.5, 0, 1");
    // ...but the stored readback is untouched.
    expect(useInspectorStore.getState().hover!.sample.rgba).toEqual([1.0, 0.5, 0.0, 1.0]);
  });

  it("renders pinned readouts that persist when the inspector is disabled, and unpins", () => {
    useInspectorStore.getState().pin({ x: 10, y: 10 }, sample([0.0, 0.0, 1.0, 1.0]));
    // Disabled, but pins still show (they persist while panning the graph).
    render(<InspectorOverlay geometry={GEOMETRY} />);
    const pin = screen.getByTestId("inspect-pin-readout");
    expect(pin).toHaveTextContent("(128, 96)");

    // The unpin button removes it.
    fireEvent.click(screen.getByRole("button", { name: "×" }));
    expect(useInspectorStore.getState().pinned).toHaveLength(0);
  });
});
