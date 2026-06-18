// The draggable split divider overlay (#60). A thin DOM line + grab handle laid
// over the preview pane; dragging it sets the normalized divider position in the
// compare store. The COMPOSITED pixels are clipped at the boundary by the canvas
// compositor (compareCompositor) — this overlay is only the visible handle, so
// the line and the pixel seam stay in lockstep (both derive from `splitPos`).
//
// Position is normalized (CSS percent), so the handle tracks the pane regardless
// of the canvas's object-fit scaling. Pointer math goes through
// `paneToNormalized`, the single pane<->normalized mapping (the #61 seam).

import { useCallback, useEffect, useRef, type RefObject } from "react";

import { paneToNormalized, type SplitOrientation } from "./compareGeometry";
import { useCompareStore } from "./compareStore";

export interface SplitDividerProps {
  /** The pane element the divider is laid over (for pointer-to-pane geometry). */
  paneRef: RefObject<HTMLDivElement | null>;
  orientation: SplitOrientation;
  /** Normalized divider position in [0,1]. */
  pos: number;
}

export function SplitDivider({ paneRef, orientation, pos }: SplitDividerProps) {
  const setSplitPos = useCompareStore((s) => s.setSplitPos);
  const draggingRef = useRef(false);

  /** Convert a pointer event to a normalized position within the pane. */
  const posFromEvent = useCallback(
    (clientX: number, clientY: number): number => {
      const pane = paneRef.current;
      if (!pane) {
        return pos;
      }
      const rect = pane.getBoundingClientRect();
      return orientation === "vertical"
        ? paneToNormalized(clientX - rect.left, rect.width, "vertical")
        : paneToNormalized(clientY - rect.top, rect.height, "horizontal");
    },
    [paneRef, orientation, pos],
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

  const percent = `${Math.min(Math.max(pos, 0), 1) * 100}%`;
  const style: React.CSSProperties =
    orientation === "vertical"
      ? { left: percent, top: 0, bottom: 0, width: 0 }
      : { top: percent, left: 0, right: 0, height: 0 };

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
