// Pure geometry for the A/B compare + split-view compositor (#60).
//
// The compare feature draws TWO same-size images (the captured REFERENCE frame
// and the LIVE frame) onto one preview canvas, and — in split mode — clips them
// against a draggable divider so the reference fills one side and the live frame
// the other, meeting EXACTLY at the boundary.
//
// This module is the math only (no DOM, no React): it converts a normalized
// divider position into the exact integer pixel rectangles each side occupies at
// the canvas pixel size, and maps a pointer position over the pane back to a
// normalized divider position. Keeping it pure makes the clip-at-the-boundary
// guarantee (acceptance) unit-testable without a GPU or a real canvas.
//
// SEAM FOR #61 (region readback / pane<->viewport mapping): `paneToNormalized`
// is the single place the pane's pixel geometry is converted to a normalized
// [0,1] coordinate. #61's on-demand region readback wants the inverse direction
// (a normalized pane coordinate -> a source-viewport pixel rect); add that
// mapping here alongside this one so all pane<->viewport math lives in one file.

/** Which axis the split divider runs along. */
export type SplitOrientation = "vertical" | "horizontal";

/** The three compare display modes. */
export type CompareMode = "live" | "reference" | "split";

/** An axis-aligned, integer pixel rectangle on the canvas. */
export interface PixelRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** The two clip rectangles for a split, at the canvas pixel size. */
export interface SplitClip {
  /** Pixels covered by the REFERENCE side (left for vertical, top for horizontal). */
  reference: PixelRect;
  /** Pixels covered by the LIVE side (right for vertical, bottom for horizontal). */
  live: PixelRect;
}

/** Clamp `value` into the inclusive `[min, max]` range. */
export function clamp(value: number, min: number, max: number): number {
  if (value < min) {
    return min;
  }
  if (value > max) {
    return max;
  }
  return value;
}

/**
 * Split a `width x height` canvas at a normalized divider position `pos` (0..1)
 * into the reference side and the live side, EXACTLY at the boundary with no gap
 * and no overlap. `pos` is the fraction of the splitting axis the reference side
 * occupies — `0.5` is a centered divider; `0` collapses reference to nothing;
 * `1` gives the whole canvas to the reference.
 *
 * The boundary is rounded to a whole pixel (`Math.round`) so the two rectangles
 * tile the canvas with no sub-pixel seam, and `reference.width + live.width`
 * (vertical) — or the heights (horizontal) — always sum to the canvas size.
 */
export function splitClip(
  width: number,
  height: number,
  pos: number,
  orientation: SplitOrientation,
): SplitClip {
  const t = clamp(pos, 0, 1);
  if (orientation === "vertical") {
    const boundary = Math.round(width * t);
    return {
      reference: { x: 0, y: 0, width: boundary, height },
      live: { x: boundary, y: 0, width: width - boundary, height },
    };
  }
  const boundary = Math.round(height * t);
  return {
    reference: { x: 0, y: 0, width, height: boundary },
    live: { x: 0, y: boundary, width, height: height - boundary },
  };
}

/**
 * The pixel coordinate (along the split axis, in canvas space) the divider line
 * sits at for a normalized `pos` — i.e. where to draw the divider handle. Matches
 * the boundary `splitClip` uses, so the visible line lands exactly on the clip
 * seam.
 */
export function dividerPixel(
  width: number,
  height: number,
  pos: number,
  orientation: SplitOrientation,
): number {
  const t = clamp(pos, 0, 1);
  return orientation === "vertical" ? Math.round(width * t) : Math.round(height * t);
}

/**
 * Map a pointer position over the pane (in the pane's own CSS pixels, relative to
 * the pane's top-left) back to a normalized divider position in `[0,1]`. The pane
 * may be a different size than the canvas (the canvas is `object-fit: contain`
 * scaled), but the divider position is normalized so only the pane's own extent
 * matters. Returns a clamped value so a drag past the edge pins to 0 or 1.
 */
export function paneToNormalized(
  offset: number,
  extent: number,
  orientation: SplitOrientation,
): number {
  void orientation; // axis is selected by the caller via which offset/extent it passes
  if (extent <= 0) {
    return 0.5;
  }
  return clamp(offset / extent, 0, 1);
}
