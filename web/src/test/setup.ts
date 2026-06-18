// Vitest setup (#45): register the @testing-library/jest-dom matchers (e.g.
// toBeInTheDocument, toHaveTextContent) and auto-clean the rendered DOM between
// tests so component tests don't leak nodes into each other. Referenced from
// vite.config.ts `test.setupFiles`.
import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

// jsdom ships no canvas raster and no `ImageData` constructor, but the preview
// compare code (#60) constructs `ImageData` for the live/reference frames. Provide
// a minimal, spec-shaped polyfill (data + width + height) so those modules and
// their tests can run under jsdom without a real canvas backend.
if (typeof globalThis.ImageData === "undefined") {
  class ImageDataPolyfill {
    readonly data: Uint8ClampedArray;
    readonly width: number;
    readonly height: number;
    constructor(
      dataOrWidth: Uint8ClampedArray | number,
      widthOrHeight: number,
      height?: number,
    ) {
      if (typeof dataOrWidth === "number") {
        this.width = dataOrWidth;
        this.height = widthOrHeight;
        this.data = new Uint8ClampedArray(this.width * this.height * 4);
      } else {
        this.data = dataOrWidth;
        this.width = widthOrHeight;
        this.height = height ?? dataOrWidth.length / 4 / widthOrHeight;
      }
    }
  }
  globalThis.ImageData = ImageDataPolyfill as unknown as typeof ImageData;
}

afterEach(() => {
  cleanup();
});
