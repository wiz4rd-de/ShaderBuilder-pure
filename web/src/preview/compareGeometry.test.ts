import { describe, expect, it } from "vitest";

import {
  clamp,
  dividerPixel,
  paneToNormalized,
  splitClip,
} from "./compareGeometry";

describe("clamp", () => {
  it("pins below/above the range and passes through inside", () => {
    expect(clamp(-0.5, 0, 1)).toBe(0);
    expect(clamp(1.5, 0, 1)).toBe(1);
    expect(clamp(0.42, 0, 1)).toBe(0.42);
  });
});

describe("splitClip (vertical)", () => {
  it("tiles the canvas with no gap and no overlap", () => {
    const { reference, live } = splitClip(512, 384, 0.5, "vertical");
    // The two rects abut exactly at the boundary...
    expect(reference.x).toBe(0);
    expect(reference.width).toBe(256);
    expect(live.x).toBe(256); // live starts exactly where reference ends
    // ...and together cover the full width with no double-counted pixel.
    expect(reference.width + live.width).toBe(512);
    // Both span the full height for a vertical divider.
    expect(reference.height).toBe(384);
    expect(live.height).toBe(384);
  });

  it("rounds the boundary to a whole pixel but still tiles exactly", () => {
    // 512 * 0.3 = 153.6 -> rounds to 154; live takes the remainder.
    const { reference, live } = splitClip(512, 384, 0.3, "vertical");
    expect(reference.width).toBe(154);
    expect(live.x).toBe(154);
    expect(live.width).toBe(512 - 154);
    expect(reference.width + live.width).toBe(512);
  });

  it("pos=0 collapses the reference side to zero width", () => {
    const { reference, live } = splitClip(512, 384, 0, "vertical");
    expect(reference.width).toBe(0);
    expect(live.x).toBe(0);
    expect(live.width).toBe(512);
  });

  it("pos=1 gives the whole canvas to the reference", () => {
    const { reference, live } = splitClip(512, 384, 1, "vertical");
    expect(reference.width).toBe(512);
    expect(live.width).toBe(0);
  });

  it("clamps an out-of-range position", () => {
    expect(splitClip(512, 384, 2, "vertical").reference.width).toBe(512);
    expect(splitClip(512, 384, -1, "vertical").reference.width).toBe(0);
  });
});

describe("splitClip (horizontal)", () => {
  it("tiles top/bottom with no gap and full width on each", () => {
    const { reference, live } = splitClip(512, 384, 0.5, "horizontal");
    expect(reference.y).toBe(0);
    expect(reference.height).toBe(192);
    expect(live.y).toBe(192);
    expect(reference.height + live.height).toBe(384);
    expect(reference.width).toBe(512);
    expect(live.width).toBe(512);
  });

  it("rounds the boundary but still tiles exactly", () => {
    // 384 * 0.7 = 268.8 -> 269.
    const { reference, live } = splitClip(512, 384, 0.7, "horizontal");
    expect(reference.height).toBe(269);
    expect(live.y).toBe(269);
    expect(reference.height + live.height).toBe(384);
  });
});

describe("dividerPixel", () => {
  it("matches the clip boundary so the line lands on the seam", () => {
    expect(dividerPixel(512, 384, 0.3, "vertical")).toBe(
      splitClip(512, 384, 0.3, "vertical").reference.width,
    );
    expect(dividerPixel(512, 384, 0.7, "horizontal")).toBe(
      splitClip(512, 384, 0.7, "horizontal").reference.height,
    );
  });
});

describe("paneToNormalized", () => {
  it("maps an offset within the extent to a fraction", () => {
    expect(paneToNormalized(50, 200, "vertical")).toBeCloseTo(0.25, 6);
    expect(paneToNormalized(150, 200, "horizontal")).toBeCloseTo(0.75, 6);
  });

  it("clamps a drag past either edge", () => {
    expect(paneToNormalized(-20, 200, "vertical")).toBe(0);
    expect(paneToNormalized(260, 200, "vertical")).toBe(1);
  });

  it("returns the center for a degenerate (zero) extent", () => {
    expect(paneToNormalized(0, 0, "vertical")).toBe(0.5);
  });
});
