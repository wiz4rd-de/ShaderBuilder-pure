// A/B compare + split-view UI state for the preview pane (#60).
//
// DESIGN: the reference frame is a snapshot of the LAST DECODED LIVE FRAME, taken
// on the FRONTEND when the user hits "set reference". This is the simplest
// correct option for the acceptance ("capture a reference, change a parameter,
// see the difference"): it keeps the render engine SINGLE-STREAM and never blocks
// the render thread (read_back is a blocking device.poll — never call it per
// frame just to compare), and it works against the existing one-channel frame
// pump with no protocol change. An engine-side `capture_reference` command was
// considered for spec-parity on the command surface, but a frontend snapshot
// fully satisfies acceptance, so we keep the engine untouched. (See #61 for the
// on-demand region-readback seam, which is where an engine capture would land if
// ever needed.)
//
// CRITICAL: this state is UI-ONLY and lives OUTSIDE documentStore, so toggling
// compare modes or dragging the divider NEVER marks the project dirty (task 5).
// The captured `reference` is an ImageData (RGBA8 pixels at the frame's size),
// not the project — it is pure ephemera, cleared on demand.
import { create } from "zustand";

import type { CompareMode, SplitOrientation } from "./compareGeometry";

export interface CompareState {
  /** Which image the pane shows: live only, the reference, or a split of both. */
  mode: CompareMode;
  /** The divider orientation in split mode. */
  orientation: SplitOrientation;
  /** Normalized divider position in [0,1] — fraction the reference side covers. */
  splitPos: number;
  /**
   * The captured reference frame (RGBA8 ImageData at the frame's native size), or
   * `null` when nothing is captured. Held here, never in the document. With this
   * `null` the compare controls are inert (`hasReference` is false).
   */
  reference: ImageData | null;

  /** Whether a reference has been captured (drives enabling the compare controls). */
  hasReference: () => boolean;

  /**
   * Capture `frame` as the reference (a defensive copy, so the live pixel buffer
   * can keep mutating). Does NOT change the mode — the caller decides whether to
   * flip to a compare view after capturing. No-op-safe with a null frame.
   */
  setReference: (frame: ImageData | null) => void;
  /** Clear the reference and fall back to the live view. */
  clearReference: () => void;

  /** Switch the compare mode (instant flip; no engine round-trip). */
  setMode: (mode: CompareMode) => void;
  /** Move the split divider (normalized [0,1]); clamped by the caller's geometry. */
  setSplitPos: (pos: number) => void;
  /** Flip the divider orientation. */
  setOrientation: (orientation: SplitOrientation) => void;
}

/** Defensive copy of an ImageData so the captured reference is decoupled. */
function copyImageData(src: ImageData): ImageData {
  // Copy the pixel bytes into a fresh buffer; the source buffer is the live frame
  // and will be overwritten by the next decoded frame.
  const copy = new Uint8ClampedArray(src.data.length);
  copy.set(src.data);
  return new ImageData(copy, src.width, src.height);
}

export const useCompareStore = create<CompareState>((set, get) => ({
  mode: "live",
  orientation: "vertical",
  splitPos: 0.5,
  reference: null,

  hasReference: () => get().reference !== null,

  setReference: (frame) =>
    set({ reference: frame ? copyImageData(frame) : null }),

  clearReference: () => set({ reference: null, mode: "live" }),

  setMode: (mode) => set({ mode }),
  setSplitPos: (pos) => set({ splitPos: pos }),
  setOrientation: (orientation) => set({ orientation }),
}));
