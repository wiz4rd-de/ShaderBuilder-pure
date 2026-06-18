// The draggable split divider overlay (#60, contain-corrected #1). A thin DOM line
// + grab handle laid over the preview canvas; dragging it sets the normalized
// divider position in the compare store. The COMPOSITED pixels are clipped at the
// boundary by the canvas compositor (compareCompositor) at `splitClip`'s
// `Math.round(canvas*pos)` seam.
//
// The canvas renders `object-fit: contain` (App.css), so its bitmap is letterboxed
// inside the element box when the pane aspect != the canvas aspect. The divider is
// therefore CONTAIN-AWARE: pointer math and line placement both go through the
// SHARED contain helpers in pixelInspect (`domToSplitNormalized` /
// `splitSeamBoxOffset` / `containRect`) — the SAME math the pixel inspector uses —
// so the visible line lands exactly on the pixel seam for ANY pane aspect, not just
// 4:3 (#1).

import { useCallback, useEffect, useRef, type RefObject } from "react";

import type { SplitOrientation } from "./compareGeometry";
import { useCompareStore } from "./compareStore";
import type { CanvasGeometry } from "./InspectorOverlay";
import { domToSplitNormalized, splitSeamBoxOffset } from "./pixelInspect";

export interface SplitDividerProps {
  /** The canvas element the divider is laid over (for pointer-to-canvas geometry). */
  canvasRef: RefObject<HTMLCanvasElement | null>;
  /** The canvas's measured contain geometry (box size, canvas px, offset in pane). */
  geometry: CanvasGeometry;
  orientation: SplitOrientation;
  /** Normalized divider position in [0,1]. */
  pos: number;
}

export function SplitDivider({ canvasRef, geometry, orientation, pos }: SplitDividerProps) {
  const setSplitPos = useCompareStore((s) => s.setSplitPos);
  const draggingRef = useRef(false);

  /**
   * Convert a pointer event to a normalized position over the RENDERED image,
   * undoing the `object-fit: contain` letterbox via the shared contain math (#1),
   * so a drag maps to the same space as the composited pixel seam.
   */
  const posFromEvent = useCallback(
    (clientX: number, clientY: number): number => {
      const canvas = canvasRef.current;
      if (!canvas) {
        return pos;
      }
      const rect = canvas.getBoundingClientRect();
      const offset = orientation === "vertical" ? clientX - rect.left : clientY - rect.top;
      return domToSplitNormalized(
        offset,
        rect.width,
        rect.height,
        canvas.width,
        canvas.height,
        orientation,
      );
    },
    [canvasRef, orientation, pos],
  );

  // Window-level pointer listeners during a drag so the divider keeps tracking
  // even when the pointer leaves the thin handle (smooth drag — acceptance).
  useEffect(() => {
    function onMove(e: PointerEvent) {
      if (!draggingRef.current) {
        return;
      }
      e.preventDefault();
      setSplitPos(posFromEvent(e.clientX, e.clientY));
    }
    function onUp() {
      draggingRef.current = false;
    }
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };
  }, [posFromEvent, setSplitPos]);

  const onPointerDown = useCallback(
    (e: React.PointerEvent) => {
      e.preventDefault();
      draggingRef.current = true;
      // Jump the divider to the press point (also handles a click-to-position).
      setSplitPos(posFromEvent(e.clientX, e.clientY));
    },
    [posFromEvent, setSplitPos],
  );

  // Place the line on the SEAM: the canvas-pixel boundary (rounded exactly as the
  // compositor) mapped back into box CSS space, plus the canvas's offset within the
  // positioning parent. Line and pixel seam now share one space (#1).
  const seamOffset = splitSeamBoxOffset(
    pos,
    geometry.boxWidth,
    geometry.boxHeight,
    geometry.canvasWidth,
    geometry.canvasHeight,
    orientation,
  );
  const style: React.CSSProperties =
    orientation === "vertical"
      ? { left: `${geometry.offsetLeft + seamOffset}px`, top: 0, bottom: 0, width: 0 }
      : { top: `${geometry.offsetTop + seamOffset}px`, left: 0, right: 0, height: 0 };

  return (
    <div
      className={`preview__divider preview__divider--${orientation}`}
      style={style}
      role="separator"
      aria-orientation={orientation === "vertical" ? "vertical" : "horizontal"}
      aria-label="Split divider"
      onPointerDown={onPointerDown}
      data-testid="split-divider"
    >
      <span className="preview__divider-handle" aria-hidden="true" />
    </div>
  );
}
