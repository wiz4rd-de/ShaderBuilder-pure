// Pixel-inspector UI state for the preview pane (#61, Spec §8.5).
//
// Like the compare store (#60), this is UI-ONLY state held OUTSIDE documentStore
// so enabling the inspector, hovering, pinning, or toggling the readout display
// NEVER marks the project dirty. It holds: whether the inspector is on, the
// display toggle (0-255/0-1 and sRGB/linear — DISPLAY only, never the readback),
// the current hovered sample, and the list of PINNED samples (click-to-pin, with
// clear/unpin) that persist while the user pans the graph.
//
// The actual readback is an async Tauri `inspect_pixel` round-trip owned by the
// PreviewCanvas (it knows the pane geometry); this store only retains the results.
import { create } from "zustand";

import type { PixelSample } from "../bindings/PixelSample";
import type { ReadoutOptions } from "./pixelInspect";
import type { CanvasPixel } from "./pixelInspect";

/** A pinned sample: the readback plus the pane (canvas) pixel it was taken at, so
 * the crosshair stays anchored to that spot on the pane. */
export interface PinnedSample {
  /** A stable id for list keys / unpinning. */
  id: number;
  /** The pane (canvas) pixel the sample was taken at — anchors the crosshair. */
  pane: CanvasPixel;
  /** The readback at that pixel (simulated-viewport coord + RGBA). */
  sample: PixelSample;
}

export interface InspectorState {
  /** Whether the inspector overlay is active (hover crosshair + readout). */
  enabled: boolean;
  /** Display toggle: 0-255 bytes vs 0-1 floats, sRGB vs linear (DISPLAY only). */
  display: ReadoutOptions;
  /**
   * The current hovered sample + the pane pixel it was taken at, or `null` when the
   * pointer is off the content (a letterbox margin) or no probe has returned yet.
   */
  hover: { pane: CanvasPixel; sample: PixelSample } | null;
  /** The click-pinned samples (persist while panning; cleared on demand). */
  pinned: PinnedSample[];

  /** Toggle the inspector on/off. Disabling clears the transient hover (pins stay). */
  setEnabled: (enabled: boolean) => void;
  /** Toggle 0-255 bytes vs 0-1 floats display. */
  setBytes: (bytes: boolean) => void;
  /** Toggle sRGB-encoded vs linear display. */
  setSrgb: (srgb: boolean) => void;
  /** Record the latest hovered sample (or clear it when off the content). */
  setHover: (hover: { pane: CanvasPixel; sample: PixelSample } | null) => void;
  /** Pin a sample at a pane pixel (click-to-pin). */
  pin: (pane: CanvasPixel, sample: PixelSample) => void;
  /** Remove one pinned sample by id (unpin). */
  unpin: (id: number) => void;
  /** Clear all pinned samples. */
  clearPins: () => void;
}

/** Monotonic id source for pinned samples. */
let nextPinId = 1;

export const useInspectorStore = create<InspectorState>((set) => ({
  enabled: false,
  display: { bytes: true, srgb: false },
  hover: null,
  pinned: [],

  setEnabled: (enabled) =>
    set((s) => ({ enabled, hover: enabled ? s.hover : null })),
  setBytes: (bytes) => set((s) => ({ display: { ...s.display, bytes } })),
  setSrgb: (srgb) => set((s) => ({ display: { ...s.display, srgb } })),
  setHover: (hover) => set({ hover }),
  pin: (pane, sample) =>
    set((s) => ({
      pinned: [...s.pinned, { id: nextPinId++, pane, sample }],
    })),
  unpin: (id) => set((s) => ({ pinned: s.pinned.filter((p) => p.id !== id) })),
  clearPins: () => set({ pinned: [] }),
}));
