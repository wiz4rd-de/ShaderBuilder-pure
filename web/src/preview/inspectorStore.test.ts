import { beforeEach, describe, expect, it } from "vitest";

import type { PixelSample } from "../bindings/PixelSample";
import { useInspectorStore } from "./inspectorStore";

function sample(x: number, y: number): PixelSample {
  return {
    inside: true,
    x,
    y,
    viewportWidth: 256,
    viewportHeight: 224,
    rgba: [0.5, 0.25, 0.1, 1.0],
    format: "rgba8Unorm",
  };
}

describe("inspectorStore", () => {
  beforeEach(() => {
    // Reset to a known state between tests (the store is a module singleton).
    useInspectorStore.setState({
      enabled: false,
      display: { bytes: true, srgb: false },
      hover: null,
      pinned: [],
    });
  });

  it("toggling off clears the transient hover but keeps pins", () => {
    const s = useInspectorStore.getState();
    s.setEnabled(true);
    s.setHover({ pane: { x: 10, y: 12 }, sample: sample(20, 24) });
    s.pin({ x: 30, y: 40 }, sample(60, 80));
    expect(useInspectorStore.getState().hover).not.toBeNull();
    expect(useInspectorStore.getState().pinned).toHaveLength(1);

    s.setEnabled(false);
    // Hover is transient — cleared; the pin persists (acceptance: pins survive).
    expect(useInspectorStore.getState().hover).toBeNull();
    expect(useInspectorStore.getState().pinned).toHaveLength(1);
  });

  it("the display toggle does not touch hover/pinned readbacks", () => {
    const s = useInspectorStore.getState();
    s.setHover({ pane: { x: 1, y: 1 }, sample: sample(2, 2) });
    const before = useInspectorStore.getState().hover!.sample.rgba;

    s.setBytes(false);
    s.setSrgb(true);
    expect(useInspectorStore.getState().display).toEqual({ bytes: false, srgb: true });
    // The stored sample is untouched — only display options changed.
    expect(useInspectorStore.getState().hover!.sample.rgba).toEqual(before);
  });

  it("pins accumulate with stable unique ids and unpin removes one", () => {
    const s = useInspectorStore.getState();
    s.pin({ x: 1, y: 1 }, sample(1, 1));
    s.pin({ x: 2, y: 2 }, sample(2, 2));
    const pins = useInspectorStore.getState().pinned;
    expect(pins).toHaveLength(2);
    expect(pins[0].id).not.toBe(pins[1].id);

    s.unpin(pins[0].id);
    const after = useInspectorStore.getState().pinned;
    expect(after).toHaveLength(1);
    expect(after[0].id).toBe(pins[1].id);
  });

  it("clearPins removes every pin", () => {
    const s = useInspectorStore.getState();
    s.pin({ x: 1, y: 1 }, sample(1, 1));
    s.pin({ x: 2, y: 2 }, sample(2, 2));
    s.clearPins();
    expect(useInspectorStore.getState().pinned).toHaveLength(0);
  });
});
