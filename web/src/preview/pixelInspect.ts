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
  if (boxWidth <= 0 || boxHeight <= 0 || canvasWidth <= 0 || canvasHeight <= 0) {
    return null;
  }
  // `object-fit: contain`: scale by the tighter axis so the whole canvas fits.
  const scale = Math.min(boxWidth / canvasWidth, boxHeight / canvasHeight);
  const renderedW = canvasWidth * scale;
  const renderedH = canvasHeight * scale;
  // Centered: equal margins on the looser axis.
  const marginX = (boxWidth - renderedW) / 2;
  const marginY = (boxHeight - renderedH) / 2;
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
