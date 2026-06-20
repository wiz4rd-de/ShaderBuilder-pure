import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";

import type { PixelSample } from "../bindings/PixelSample";
import { InspectorControls } from "./InspectorControls";
import { useInspectorStore } from "./inspectorStore";

function sample(): PixelSample {
  return {
    inside: true,
    x: 1,
    y: 1,
    viewportWidth: 256,
    viewportHeight: 224,
    rgba: [0.5, 0.25, 0.0, 1.0],
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

describe("InspectorControls", () => {
  it("toggles the inspector on/off", () => {
    render(<InspectorControls />);
    const toggle = screen.getByRole("button", { name: "Inspect" });
    expect(toggle).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(toggle);
    expect(useInspectorStore.getState().enabled).toBe(true);
    expect(toggle).toHaveAttribute("aria-pressed", "true");
  });

  it("switches the 0-255 / 0-1 and linear / sRGB display toggles", () => {
    render(<InspectorControls />);
    fireEvent.click(screen.getByRole("radio", { name: "0–1" }));
    expect(useInspectorStore.getState().display.bytes).toBe(false);
    fireEvent.click(screen.getByRole("radio", { name: "sRGB" }));
    expect(useInspectorStore.getState().display.srgb).toBe(true);
  });

  it("Clear pins is disabled with no pins", () => {
    render(<InspectorControls />);
    expect(screen.getByRole("button", { name: /Clear pins/ })).toBeDisabled();
  });

  it("Clear pins removes them when present", () => {
    useInspectorStore.getState().pin({ x: 1, y: 1 }, sample());
    render(<InspectorControls />);
    const clear = screen.getByRole("button", { name: /Clear pins/ });
    expect(clear).not.toBeDisabled();
    fireEvent.click(clear);
    expect(useInspectorStore.getState().pinned).toHaveLength(0);
  });
});
