// Pure geometry + formatting for the pixel inspector overlay (#61, Spec §8.5).
//
// The inspector lets the user hover/click the preview pane to read a pixel's
// SIMULATED-VIEWPORT coordinate + RGBA. Two coordinate hops are involved:
//
//   1. DOM → CANVAS PIXEL (this module): the pointer's position over the
//      `<canvas>` element's box maps to a canvas pixel. The canvas is drawn
//      `object-fit: contain`, so the rendered image is letterboxed WITHIN the
//      element's box when the box aspect ≠ the canvas-pixel aspect — `domToCanvasPixel`
//      undoes that fit. The canvas pixel == the PANE pixel (the canvas is sized to
//      the pane).
//   2. PANE → SIMULATED-VIEWPORT (the Rust `inspect_pixel` command): the pane pixel
//      maps through the §9 content rect to a viewport pixel; letterbox bars report
//      "outside". That hop is the engine's (it owns the viewport/source state).
//
// Keeping (1) and the readout formatting pure makes them unit-testable without a
// real canvas or GPU — the acceptance ("hovering a known pattern reports the
// expected RGBA at known coordinates"; "the sRGB toggle changes the DISPLAYED
// value, not the readback") is exercised here.

import type { PixelSample } from "../bindings/PixelSample";

/** A canvas pixel coordinate (integer), == a pane pixel. */
export interface CanvasPixel {
  x: number;
  y: number;
}

/**
 * The `object-fit: contain` placement of a canvas inside its element box: the
 * uniform `scale`, the rendered image size, and the centering margins (CSS px).
 * `null` when the box or canvas is degenerate. This is THE single source of the
 * contain math — the pixel inspector AND the split divider (#1) both derive their
 * coordinate mapping from it so the crosshair, the divider line, and the pixel
 * seam can never diverge.
 */
export interface ContainRect {
  scale: number;
  renderedW: number;
  renderedH: number;
  marginX: number;
  marginY: number;
}

/**
 * Compute the `object-fit: contain` rect of a `canvasWidth × canvasHeight` canvas
 * laid out in a `boxWidth × boxHeight` element box: scaled by the tighter axis and
 * centered, leaving equal margins on the looser axis. Returns `null` for a
 * degenerate box/canvas.
 */
export function containRect(
  boxWidth: number,
  boxHeight: number,
  canvasWidth: number,
  canvasHeight: number,
): ContainRect | null {
  if (boxWidth <= 0 || boxHeight <= 0 || canvasWidth <= 0 || canvasHeight <= 0) {
    return null;
  }
  const scale = Math.min(boxWidth / canvasWidth, boxHeight / canvasHeight);
  const renderedW = canvasWidth * scale;
  const renderedH = canvasHeight * scale;
  return {
    scale,
    renderedW,
    renderedH,
    marginX: (boxWidth - renderedW) / 2,
    marginY: (boxHeight - renderedH) / 2,
  };
}

/** Clamp `value` into the inclusive `[min, max]` range. */
function clamp(value: number, min: number, max: number): number {
  return value < min ? min : value > max ? max : value;
}

/**
 * Map a pointer position over the `<canvas>` element to a CANVAS PIXEL (== pane
 * pixel), undoing the `object-fit: contain` letterbox.
 *
 * `offsetX/offsetY` are the pointer position relative to the element's top-left
 * (CSS px); `boxWidth/boxHeight` are the element's displayed box (CSS px);
 * `canvasWidth/canvasHeight` are the canvas's backing pixel size. With
 * `object-fit: contain` the canvas content is scaled by the SMALLER axis ratio and
 * centered, leaving equal margins on the looser axis — this reverses that to find
 * which canvas pixel the pointer is over.
 *
 * Returns `null` when the pointer is in the contain margin (outside the rendered
 * image entirely) so the caller reports no sample, or when the box is degenerate.
 */
export function domToCanvasPixel(
  offsetX: number,
  offsetY: number,
  boxWidth: number,
  boxHeight: number,
  canvasWidth: number,
  canvasHeight: number,
): CanvasPixel | null {
  const rect = containRect(boxWidth, boxHeight, canvasWidth, canvasHeight);
  if (!rect) {
    return null;
  }
  // `object-fit: contain`: scale by the tighter axis, centered with equal margins.
  const { scale, renderedW, renderedH, marginX, marginY } = rect;
  const localX = offsetX - marginX;
  const localY = offsetY - marginY;
  // In the contain margin (outside the rendered image): no canvas pixel.
  if (localX < 0 || localY < 0 || localX >= renderedW || localY >= renderedH) {
    return null;
  }
  // Back into canvas-pixel space, floored to a whole pixel and clamped to the
  // last valid index (a pointer exactly on the far edge rounds in).
  const x = clamp(Math.floor(localX / scale), 0, canvasWidth - 1);
  const y = clamp(Math.floor(localY / scale), 0, canvasHeight - 1);
  return { x, y };
}

/** How the inspector readout numbers are displayed (a UI-only toggle, #61). */
export interface ReadoutOptions {
  /**
   * `true` → show 0..255 integer channels; `false` → show 0..1 floats. Changes
   * only the DISPLAY, never the readback value.
   */
  bytes: boolean;
  /**
   * `true` → apply the linear→sRGB OETF before display (gamma-encoded values);
   * `false` → show the raw linear value. Changes only the DISPLAY.
   */
  srgb: boolean;
}

/**
 * The linear→sRGB transfer function (IEC 61966-2-1 OETF), applied per channel when
 * the sRGB display toggle is on. Operates on a normalized `0..1` (or extended, for
 * float targets) linear value. The alpha channel is NOT gamma-encoded by sRGB, so
 * callers pass alpha through `linearToSrgb` is avoided for A.
 */
export function linearToSrgb(c: number): number {
  if (c <= 0) {
    return 0;
  }
  // sRGB is only defined on [0,1]; an extended (HDR) value passes through unchanged
  // so the displayed number still reflects the out-of-range readback (float targets)
  // rather than clipping it to 1.
  if (c > 1) {
    return c;
  }
  return c <= 0.0031308 ? c * 12.92 : 1.055 * Math.pow(c, 1 / 2.4) - 0.055;
}

/**
 * Format one channel value for display per the toggle: optionally sRGB-encode
 * (RGB only — alpha is never gamma-encoded), then either scale to 0..255 (rounded)
 * or keep as a 0..1 float (3 decimals). An out-of-range (HDR) value is preserved
 * rather than clamped so float targets read their true value.
 */
export function formatChannel(
  value: number,
  isAlpha: boolean,
  opts: ReadoutOptions,
): string {
  const display = opts.srgb && !isAlpha ? linearToSrgb(value) : value;
  if (opts.bytes) {
    return String(Math.round(display * 255));
  }
  // Trim trailing zeros for a compact float (0.5 not 0.500), keeping HDR sign.
  return parseFloat(display.toFixed(3)).toString();
}

/**
 * Format a [`PixelSample`] RGBA for display per the toggle (#61). Returns the four
 * channel strings in R,G,B,A order. The raw sample is never mutated — only the
 * displayed text changes with the toggle.
 */
export function formatRgba(sample: PixelSample, opts: ReadoutOptions): [string, string, string, string] {
  const [r, g, b, a] = sample.rgba;
  return [
    formatChannel(r, false, opts),
    formatChannel(g, false, opts),
    formatChannel(b, false, opts),
    formatChannel(a, true, opts),
  ];
}

/** The viewport-coordinate readout, e.g. `"(128, 96)"`. */
export function formatCoord(sample: PixelSample): string {
  return `(${sample.x}, ${sample.y})`;
}

/** A CSS position (px) within the canvas element's box. */
export interface BoxPosition {
  left: number;
  top: number;
}

/**
 * The inverse of [`domToCanvasPixel`] for crosshair placement (#61): the CSS
 * position (relative to the canvas element's box top-left) of a CANVAS PIXEL's
 * CENTER, accounting for the `object-fit: contain` scale + centering margin. Used
 * to anchor the hover/pin crosshairs over the rendered image.
 */
export function canvasPixelToBoxPosition(
  px: number,
  py: number,
  boxWidth: number,
  boxHeight: number,
  canvasWidth: number,
  canvasHeight: number,
): BoxPosition {
  const rect = containRect(boxWidth, boxHeight, canvasWidth, canvasHeight);
  if (!rect) {
    return { left: 0, top: 0 };
  }
  const { scale, marginX, marginY } = rect;
  // The pixel's CENTER (`+0.5`) in box CSS space.
  return {
    left: marginX + (px + 0.5) * scale,
    top: marginY + (py + 0.5) * scale,
  };
}

/**
 * Map a pointer offset along ONE axis (CSS px, relative to the canvas element's
 * box top-left) to a normalized divider position in `[0,1]` over the RENDERED
 * image (#1), undoing the `object-fit: contain` letterbox via the shared
 * [`containRect`]. `vertical` uses the X axis (box width / canvas width);
 * `horizontal` uses Y. A pointer in the contain margin pins to 0 or 1. Shares the
 * contain math with the pixel inspector so the divider line and the pixel seam can
 * never diverge from the crosshair.
 */
export function domToSplitNormalized(
  offset: number,
  boxWidth: number,
  boxHeight: number,
  canvasWidth: number,
  canvasHeight: number,
  orientation: "vertical" | "horizontal",
): number {
  const rect = containRect(boxWidth, boxHeight, canvasWidth, canvasHeight);
  if (!rect) {
    return 0.5;
  }
  const margin = orientation === "vertical" ? rect.marginX : rect.marginY;
  const rendered = orientation === "vertical" ? rect.renderedW : rect.renderedH;
  return clamp((offset - margin) / rendered, 0, 1);
}

/**
 * The CSS-px offset (relative to the canvas element's box top-left) of the split
 * SEAM for a normalized position `pos` (#1), so the visible divider line lands
 * EXACTLY on the compositor's pixel seam (`splitClip`'s `Math.round(canvas*pos)`).
 * `vertical` returns a left offset, `horizontal` a top offset. Built from the same
 * [`containRect`] as the inspector + pointer mapping (one shared space).
 */
export function splitSeamBoxOffset(
  pos: number,
  boxWidth: number,
  boxHeight: number,
  canvasWidth: number,
  canvasHeight: number,
  orientation: "vertical" | "horizontal",
): number {
  const rect = containRect(boxWidth, boxHeight, canvasWidth, canvasHeight);
  if (!rect) {
    return 0;
  }
  const t = clamp(pos, 0, 1);
  // Round in CANVAS pixels to match the compositor's `splitClip` seam exactly,
  // then map that canvas-pixel boundary back into box CSS space.
  const canvasExtent = orientation === "vertical" ? canvasWidth : canvasHeight;
  const margin = orientation === "vertical" ? rect.marginX : rect.marginY;
  const seamCanvasPx = Math.round(canvasExtent * t);
  return margin + seamCanvasPx * rect.scale;
}
