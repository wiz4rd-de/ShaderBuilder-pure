// Pixel-inspector overlay for the preview pane (#61, Spec §8.5).
//
// An absolutely-positioned layer over the rendered `<canvas>` that draws a
// crosshair at the hovered pane pixel + at every PINNED pixel, plus a readout box
// (the SIMULATED-VIEWPORT coordinate + RGBA) for the hover and each pin. It is
// purely presentational: PreviewCanvas owns the pointer handlers and the async
// `inspect_pixel` round-trip and feeds results into the inspector store; this
// component just renders that state, positioned via `canvasPixelToBoxPosition`
// (the inverse of the pointer→pixel transform) so a crosshair sits exactly on its
// pane pixel regardless of the canvas's object-fit:contain scaling.
//
// Pointer events pass through (`pointer-events: none` on the layer) EXCEPT the pin
// readout's unpin button, so hovering/clicking still reaches the canvas underneath.

import { useInspectorStore } from "./inspectorStore";
import {
  canvasPixelToBoxPosition,
  formatCoord,
  formatRgba,
} from "./pixelInspect";
import type { PixelSample } from "../bindings/PixelSample";
import type { CanvasPixel } from "./pixelInspect";
import type { ReadoutOptions } from "./pixelInspect";

/** The canvas's rendered geometry, measured by PreviewCanvas, so crosshairs land
 * on the right CSS position over the object-fit:contain image. */
export interface CanvasGeometry {
  /** The canvas element's displayed box size (CSS px). */
  boxWidth: number;
  boxHeight: number;
  /** The canvas backing pixel size (== the pane size). */
  canvasWidth: number;
  canvasHeight: number;
}

export interface InspectorOverlayProps {
  /** The measured canvas geometry, or `null` before first measure. */
  geometry: CanvasGeometry | null;
}

/** A small swatch + the formatted RGBA + viewport coordinate. */
function Readout({
  sample,
  display,
}: {
  sample: PixelSample;
  display: ReadoutOptions;
}) {
  const [r, g, b, a] = formatRgba(sample, display);
  // The swatch uses the raw 0..1 linear value scaled to bytes (display-independent
  // so the swatch always shows the actual stored color).
  const [lr, lg, lb] = sample.rgba;
  const swatch = `rgb(${Math.round(lr * 255)}, ${Math.round(lg * 255)}, ${Math.round(lb * 255)})`;
  return (
    <span className="preview__inspect-readout-body">
      <span
        className="preview__inspect-swatch"
        style={{ background: swatch }}
        aria-hidden
      />
      <span className="preview__inspect-coord">{formatCoord(sample)}</span>
      <span className="preview__inspect-rgba">
        {r}, {g}, {b}, {a}
      </span>
    </span>
  );
}

/** A crosshair anchored on a pane pixel, with an attached readout. */
function Crosshair({
  pane,
  sample,
  display,
  geometry,
  pinned,
  onUnpin,
}: {
  pane: CanvasPixel;
  sample: PixelSample;
  display: ReadoutOptions;
  geometry: CanvasGeometry;
  pinned: boolean;
  onUnpin?: () => void;
}) {
  const { left, top } = canvasPixelToBoxPosition(
    pane.x,
    pane.y,
    geometry.boxWidth,
    geometry.boxHeight,
    geometry.canvasWidth,
    geometry.canvasHeight,
  );
  return (
    <div
      className={`preview__inspect-mark${pinned ? " preview__inspect-mark--pinned" : ""}`}
      style={{ left, top }}
    >
      <span className="preview__inspect-crosshair" aria-hidden />
      <span
        className="preview__inspect-readout"
        data-testid={pinned ? "inspect-pin-readout" : "inspect-hover-readout"}
      >
        <Readout sample={sample} display={display} />
        {pinned && onUnpin ? (
          <button
            type="button"
            className="preview__inspect-unpin"
            onClick={onUnpin}
            title="Remove this pinned sample"
          >
            ×
          </button>
        ) : null}
      </span>
    </div>
  );
}

export function InspectorOverlay({ geometry }: InspectorOverlayProps) {
  const enabled = useInspectorStore((s) => s.enabled);
  const hover = useInspectorStore((s) => s.hover);
  const pinned = useInspectorStore((s) => s.pinned);
  const display = useInspectorStore((s) => s.display);
  const unpin = useInspectorStore((s) => s.unpin);

  // Pins persist whenever the inspector has been used; the hover crosshair only
  // shows while the inspector is enabled and the pointer is over the content.
  if (!geometry || (!enabled && pinned.length === 0)) {
    return null;
  }

  return (
    <div className="preview__inspect-layer" data-testid="inspect-overlay">
      {pinned.map((p) => (
        <Crosshair
          key={p.id}
          pane={p.pane}
          sample={p.sample}
          display={display}
          geometry={geometry}
          pinned
          onUnpin={() => unpin(p.id)}
        />
      ))}
      {enabled && hover ? (
        <Crosshair
          pane={hover.pane}
          sample={hover.sample}
          display={display}
          geometry={geometry}
          pinned={false}
        />
      ) : null}
    </div>
  );
}
