import { describe, expect, it } from "vitest";

import type { PixelSample } from "../bindings/PixelSample";
import {
  canvasPixelToBoxPosition,
  domToCanvasPixel,
  domToSplitNormalized,
  formatChannel,
  formatCoord,
  formatRgba,
  linearToSrgb,
  splitSeamBoxOffset,
} from "./pixelInspect";
import { splitClip } from "./compareGeometry";

describe("domToCanvasPixel", () => {
  it("maps 1:1 when the box matches the canvas pixels (no contain margin)", () => {
    // Box 100x80 == canvas 100x80: identity.
    expect(domToCanvasPixel(0, 0, 100, 80, 100, 80)).toEqual({ x: 0, y: 0 });
    expect(domToCanvasPixel(50, 40, 100, 80, 100, 80)).toEqual({ x: 50, y: 40 });
    // Far edge rounds in to the last index.
    expect(domToCanvasPixel(99.9, 79.9, 100, 80, 100, 80)).toEqual({ x: 99, y: 79 });
  });

  it("undoes the object-fit:contain letterbox (box wider than the canvas aspect)", () => {
    // Canvas 100x100 in a 200x100 box: contain scales by min(200/100, 100/100)=1,
    // rendered 100x100 centered → 50px left/right margins.
    // A pointer at x=50 (the left margin edge) is the first canvas pixel.
    expect(domToCanvasPixel(50, 0, 200, 100, 100, 100)).toEqual({ x: 0, y: 0 });
    // A pointer in the left margin (x=20) is outside the rendered image.
    expect(domToCanvasPixel(20, 50, 200, 100, 100, 100)).toBeNull();
    // The right margin (x=180) is outside too.
    expect(domToCanvasPixel(180, 50, 200, 100, 100, 100)).toBeNull();
    // Center of the box maps to center of the canvas.
    expect(domToCanvasPixel(100, 50, 200, 100, 100, 100)).toEqual({ x: 50, y: 50 });
  });

  it("undoes contain when the box is scaled up (canvas smaller than the box)", () => {
    // Canvas 50x50 in a 100x100 box: contain scale = 2, no margin (square).
    expect(domToCanvasPixel(0, 0, 100, 100, 50, 50)).toEqual({ x: 0, y: 0 });
    expect(domToCanvasPixel(98, 98, 100, 100, 50, 50)).toEqual({ x: 49, y: 49 });
    // Each pair of box px maps to one canvas px.
    expect(domToCanvasPixel(10, 10, 100, 100, 50, 50)).toEqual({ x: 5, y: 5 });
  });

  it("returns null for a degenerate box or canvas", () => {
    expect(domToCanvasPixel(0, 0, 0, 80, 100, 80)).toBeNull();
    expect(domToCanvasPixel(0, 0, 100, 80, 0, 80)).toBeNull();
  });
});

describe("canvasPixelToBoxPosition (crosshair placement)", () => {
  it("places a pixel center at the right box position with a contain margin", () => {
    // Canvas 100x100 in a 200x100 box: scale 1, 50px left/right margin. Pixel (0,0)
    // center is at box-left margin + 0.5 = 50.5, top 0.5.
    const p = canvasPixelToBoxPosition(0, 0, 200, 100, 100, 100);
    expect(p.left).toBeCloseTo(50.5, 5);
    expect(p.top).toBeCloseTo(0.5, 5);
  });

  it("round-trips with domToCanvasPixel (a pixel center maps back to itself)", () => {
    const box = [200, 100] as const;
    const canvas = [100, 100] as const;
    for (const [px, py] of [
      [0, 0],
      [50, 50],
      [99, 99],
    ] as const) {
      const pos = canvasPixelToBoxPosition(px, py, box[0], box[1], canvas[0], canvas[1]);
      const back = domToCanvasPixel(pos.left, pos.top, box[0], box[1], canvas[0], canvas[1]);
      expect(back).toEqual({ x: px, y: py });
    }
  });
});

describe("linearToSrgb", () => {
  it("encodes mid-range linear with the sRGB OETF", () => {
    expect(linearToSrgb(0)).toBe(0);
    expect(linearToSrgb(1)).toBeCloseTo(1, 5);
    // 0.5 linear → ~0.735 sRGB.
    expect(linearToSrgb(0.5)).toBeCloseTo(0.7353569, 4);
    // Below the linear segment threshold uses the ×12.92 slope.
    expect(linearToSrgb(0.001)).toBeCloseTo(0.01292, 5);
  });

  it("passes extended (HDR) values through unchanged", () => {
    // A float target may store >1; the displayed value must reflect that, not clip.
    expect(linearToSrgb(2.5)).toBe(2.5);
  });
});

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

describe("formatChannel", () => {
  it("shows 0..255 bytes when bytes is on", () => {
    expect(formatChannel(1, false, { bytes: true, srgb: false })).toBe("255");
    expect(formatChannel(0.5, false, { bytes: true, srgb: false })).toBe("128");
    expect(formatChannel(0, false, { bytes: true, srgb: false })).toBe("0");
  });

  it("shows compact 0..1 floats when bytes is off", () => {
    expect(formatChannel(0.5, false, { bytes: false, srgb: false })).toBe("0.5");
    expect(formatChannel(1, false, { bytes: false, srgb: false })).toBe("1");
  });

  it("sRGB-encodes RGB but never alpha", () => {
    // 0.5 linear → ~0.7354 sRGB → 0.7354*255 ≈ 187.5 → 188 as a byte.
    expect(formatChannel(0.5, false, { bytes: true, srgb: true })).toBe("188");
    // Alpha is passed through (never gamma-encoded), so 0.5 alpha stays 128.
    expect(formatChannel(0.5, true, { bytes: true, srgb: true })).toBe("128");
  });
});

describe("formatRgba (toggle changes display, not the readback)", () => {
  it("the same sample renders differently under each toggle, sample unchanged", () => {
    const s = sample([0.5, 0.25, 0.0, 1.0]);
    const before = [...s.rgba];

    const linearBytes = formatRgba(s, { bytes: true, srgb: false });
    expect(linearBytes).toEqual(["128", "64", "0", "255"]);

    const srgbBytes = formatRgba(s, { bytes: true, srgb: true });
    // RGB gamma-encoded, alpha untouched.
    expect(srgbBytes[0]).toBe("188"); // 0.5 linear -> ~0.735 -> ~187/188
    expect(srgbBytes[3]).toBe("255");
    expect(srgbBytes).not.toEqual(linearBytes);

    const floats = formatRgba(s, { bytes: false, srgb: false });
    expect(floats).toEqual(["0.5", "0.25", "0", "1"]);

    // The raw readback never changed across the display toggles.
    expect(s.rgba).toEqual(before);
  });
});

describe("formatCoord", () => {
  it("renders the simulated-viewport coordinate", () => {
    expect(formatCoord(sample([0, 0, 0, 1]))).toBe("(128, 96)");
  });
});

describe("split divider contain-correctness (#1)", () => {
  // A NON-4:3 pane: the canvas is 512x384 (4:3) but the element box is 800x300
  // (wide/short), so object-fit:contain letterboxes with LEFT/RIGHT margins for a
  // vertical split. The bug: the divider line sat at pos*boxWidth (ignoring the
  // letterbox) while the pixel seam is at Math.round(canvasWidth*pos) — they only
  // coincided at pos=0.5 / zero margin. The fix shares the contain math.
  const box = { w: 800, h: 300 };
  const canvas = { w: 512, h: 384 };

  it("places the line exactly on the composited pixel seam for a non-4:3 pane at pos=0.25", () => {
    const pos = 0.25;

    // Where the compositor actually clips (canvas-pixel seam).
    const seamCanvasPx = splitClip(canvas.w, canvas.h, pos, "vertical").reference.width;
    expect(seamCanvasPx).toBe(Math.round(canvas.w * pos)); // 128

    // The contain rect for an 800x300 box around a 512x384 canvas: scale by the
    // tighter axis (height: 300/384), centered horizontally.
    const scale = Math.min(box.w / canvas.w, box.h / canvas.h);
    const renderedW = canvas.w * scale;
    const marginX = (box.w - renderedW) / 2;
    const expectedLinePx = marginX + seamCanvasPx * scale;

    const linePx = splitSeamBoxOffset(pos, box.w, box.h, canvas.w, canvas.h, "vertical");
    expect(linePx).toBeCloseTo(expectedLinePx, 6);

    // The naive (buggy) placement was pos*boxWidth — and for this non-4:3 pane that
    // is a DIFFERENT pixel, proving the test would fail against the old code.
    expect(linePx).not.toBeCloseTo(pos * box.w, 1);
  });

  it("pointer mapping is the inverse of the line placement (drag round-trips)", () => {
    // Dragging the pointer to the line's CSS px must recover the same normalized
    // pos (within one canvas pixel of rounding), since both go through containRect.
    const pos = 0.25;
    const linePx = splitSeamBoxOffset(pos, box.w, box.h, canvas.w, canvas.h, "vertical");
    const back = domToSplitNormalized(linePx, box.w, box.h, canvas.w, canvas.h, "vertical");
    expect(back).toBeCloseTo(pos, 2);
  });

  it("clamps a pointer in the contain margin to 0 or 1", () => {
    // Left of the rendered image (in the letterbox) pins to 0; right pins to 1.
    expect(domToSplitNormalized(0, box.w, box.h, canvas.w, canvas.h, "vertical")).toBe(0);
    expect(domToSplitNormalized(box.w, box.w, box.h, canvas.w, canvas.h, "vertical")).toBe(1);
  });
});
