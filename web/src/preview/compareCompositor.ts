// The A/B + split compositor (#60): paint the right combination of the live and
// reference frames onto the preview canvas's 2D context, clipping a split EXACTLY
// at the divider boundary at the canvas pixel size.
//
// Pure draw logic, separated from the React component so the per-frame hot path
// is allocation-light (it reuses a small scratch canvas to clip ImageData, since
// `putImageData` ignores the clip region) and so the compositing decisions can be
// reasoned about in isolation. The actual divider handle is a DOM overlay drawn
// by the React component — this module only paints pixels.

import type { CompareMode, SplitOrientation } from "./compareGeometry";
import { splitClip } from "./compareGeometry";

/**
 * A reusable scratch canvas. `putImageData` writes raw pixels and does NOT honour
 * the destination context's clip path, so to clip an ImageData to a sub-rectangle
 * we stage it on this scratch canvas and `drawImage` only the wanted rect. One
 * shared scratch avoids a per-frame canvas allocation in the hot path.
 */
let scratch: { canvas: HTMLCanvasElement; ctx: CanvasRenderingContext2D } | null = null;

function getScratch(width: number, height: number): CanvasRenderingContext2D | null {
  if (!scratch) {
    const canvas = document.createElement("canvas");
    const ctx = canvas.getContext("2d");
    if (!ctx) {
      return null;
    }
    scratch = { canvas, ctx };
  }
  if (scratch.canvas.width !== width || scratch.canvas.height !== height) {
    scratch.canvas.width = width;
    scratch.canvas.height = height;
  }
  return scratch.ctx;
}

/**
 * Draw `image` to `ctx`, showing ONLY the sub-rectangle `(sx, sy, sw, sh)` at the
 * same position on the destination — i.e. clip an ImageData to a rect. Used so a
 * split shows each frame strictly on its own side of the divider with no bleed.
 */
function drawImageRegion(
  ctx: CanvasRenderingContext2D,
  image: ImageData,
  sx: number,
  sy: number,
  sw: number,
  sh: number,
): void {
  if (sw <= 0 || sh <= 0) {
    return;
  }
  const stage = getScratch(image.width, image.height);
  if (!stage) {
    return;
  }
  stage.putImageData(image, 0, 0);
  ctx.drawImage(stage.canvas, sx, sy, sw, sh, sx, sy, sw, sh);
}

/**
 * Paint the compare composite for `mode` onto `ctx` (sized `width x height`):
 *  - "live": the live frame in full.
 *  - "reference": the captured reference in full (falls back to live if absent).
 *  - "split": reference on one side of the divider, live on the other, clipped
 *    exactly at the boundary `splitClip` computes (no gap, no overlap).
 *
 * `reference` may be `null` (nothing captured) — in that case both "reference"
 * and "split" degrade to drawing the live frame, so the pane never goes blank.
 * The two images are assumed to share the canvas size (the reference was captured
 * from this same stream); a size-mismatched reference is drawn at its own size,
 * which is fine because frames in one session are uniform.
 */
export function drawCompare(
  ctx: CanvasRenderingContext2D,
  live: ImageData,
  reference: ImageData | null,
  mode: CompareMode,
  splitPos: number,
  orientation: SplitOrientation,
  width: number,
  height: number,
): void {
  if (mode === "live" || reference === null) {
    ctx.putImageData(live, 0, 0);
    return;
  }
  if (mode === "reference") {
    ctx.putImageData(reference, 0, 0);
    return;
  }
  // split: paint live first as the base, then overlay the reference's clipped
  // side. Painting live in full first means any rounding never leaves a gap.
  const clip = splitClip(width, height, splitPos, orientation);
  ctx.putImageData(live, 0, 0);
  drawImageRegion(
    ctx,
    reference,
    clip.reference.x,
    clip.reference.y,
    clip.reference.width,
    clip.reference.height,
  );
}

/** Reset the shared scratch canvas (test isolation). */
export function __resetScratch(): void {
  scratch = null;
}
