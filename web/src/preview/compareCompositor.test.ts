import { beforeEach, describe, expect, it, vi } from "vitest";

import { __resetScratch, drawCompare } from "./compareCompositor";

/** A solid-colour ImageData of `w x h`. */
function solid(w: number, h: number, r: number, g: number, b: number): ImageData {
  const data = new Uint8ClampedArray(w * h * 4);
  for (let i = 0; i < w * h; i++) {
    data[i * 4] = r;
    data[i * 4 + 1] = g;
    data[i * 4 + 2] = b;
    data[i * 4 + 3] = 255;
  }
  return new ImageData(data, w, h);
}

/**
 * A fake 2D context that records putImageData / drawImage calls so we can assert
 * WHAT got painted WHERE without a real canvas raster. jsdom does not implement
 * canvas rendering, so we model the compositor's calls directly.
 */
function fakeCtx() {
  const puts: Array<{ image: ImageData; dx: number; dy: number }> = [];
  const draws: Array<{
    sx: number;
    sy: number;
    sw: number;
    sh: number;
    dx: number;
    dy: number;
    dw: number;
    dh: number;
  }> = [];
  const ctx = {
    putImageData: (image: ImageData, dx: number, dy: number) => {
      puts.push({ image, dx, dy });
    },
    drawImage: (
      _src: unknown,
      sx: number,
      sy: number,
      sw: number,
      sh: number,
      dx: number,
      dy: number,
      dw: number,
      dh: number,
    ) => {
      draws.push({ sx, sy, sw, sh, dx, dy, dw, dh });
    },
  } as unknown as CanvasRenderingContext2D;
  return { ctx, puts, draws };
}

beforeEach(() => {
  __resetScratch();
  // The compositor stages reference pixels on a scratch canvas; jsdom's
  // getContext returns null, so stub it to a recording context for the split path.
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockImplementation(
    () => fakeCtx().ctx as unknown as RenderingContext,
  );
});

describe("drawCompare", () => {
  const live = solid(512, 384, 0, 0, 0);
  const reference = solid(512, 384, 255, 255, 255);

  it("live mode paints only the live frame", () => {
    const { ctx, puts, draws } = fakeCtx();
    drawCompare(ctx, live, reference, "live", 0.5, "vertical", 512, 384);
    expect(puts).toEqual([{ image: live, dx: 0, dy: 0 }]);
    expect(draws).toHaveLength(0);
  });

  it("reference mode paints only the reference frame", () => {
    const { ctx, puts } = fakeCtx();
    drawCompare(ctx, live, reference, "reference", 0.5, "vertical", 512, 384);
    expect(puts).toEqual([{ image: reference, dx: 0, dy: 0 }]);
  });

  it("with no reference, every mode degrades to live (never blank)", () => {
    for (const mode of ["reference", "split"] as const) {
      const { ctx, puts } = fakeCtx();
      drawCompare(ctx, live, null, mode, 0.5, "vertical", 512, 384);
      expect(puts).toEqual([{ image: live, dx: 0, dy: 0 }]);
    }
  });

  it("split mode paints live full then overlays the reference's clipped side", () => {
    const { ctx, puts, draws } = fakeCtx();
    drawCompare(ctx, live, reference, "split", 0.5, "vertical", 512, 384);
    // Live is painted in full as the base.
    expect(puts).toContainEqual({ image: live, dx: 0, dy: 0 });
    // The reference overlay covers EXACTLY the left half, clipped at the boundary.
    expect(draws).toHaveLength(1);
    expect(draws[0]).toMatchObject({
      sx: 0,
      sy: 0,
      sw: 256, // 512 * 0.5
      sh: 384,
      dx: 0,
      dy: 0,
      dw: 256,
      dh: 384,
    });
  });

  it("split overlay region tracks the divider position", () => {
    const { ctx, draws } = fakeCtx();
    drawCompare(ctx, live, reference, "split", 0.25, "vertical", 512, 384);
    // 512 * 0.25 = 128 -> reference covers the left 128px.
    expect(draws[0]).toMatchObject({ sx: 0, sw: 128, dx: 0, dw: 128 });
  });
});
