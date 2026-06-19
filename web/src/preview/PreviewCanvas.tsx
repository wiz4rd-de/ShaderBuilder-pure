import { Channel, invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";

import type { EngineStatus } from "../bindings/EngineStatus";
import type { PixelSample } from "../bindings/PixelSample";
import { useDocumentStore } from "../store/documentStore";
import { CompareControls } from "./CompareControls";
import { drawCompare } from "./compareCompositor";
import { useCompareStore } from "./compareStore";
import { InspectorControls } from "./InspectorControls";
import { InspectorOverlay } from "./InspectorOverlay";
import type { CanvasGeometry } from "./InspectorOverlay";
import { useInspectorStore } from "./inspectorStore";
import { domToCanvasPixel } from "./pixelInspect";
import { SplitDivider } from "./SplitDivider";
import { parseFrame, toArrayBuffer } from "./frame";

/** The label + class suffix for each engine status (#62), badged on the preview. */
const STATUS_LABEL: Record<EngineStatus, string> = {
  live: "Live",
  lastGood: "Last good",
  stopped: "Render stopped",
};

const PREVIEW_WIDTH = 512;
const PREVIEW_HEIGHT = 384;

/**
 * Hosts the preview `<canvas>` and drives the Rust → webview binary frame
 * stream (Architecture §F). The frames are the offscreen wgpu render read back
 * and downsampled to the pane: `load_shader` + `load_source` (null paths => the
 * built-in passthrough over a test pattern) kick the real render once the
 * stream is up. File pickers that pass real `.slang`/image paths arrive later.
 *
 * A/B compare + split-view (#60) composite on TOP of this single stream: the
 * latest decoded frame is retained in a ref, and every frame (and every compare
 * state change) re-paints the canvas via `drawCompare` — so flipping live ⟷
 * reference or dragging the split divider is INSTANT and never touches the render
 * thread. The captured reference is a frontend ImageData snapshot held in the
 * compare store (UI-only; never dirties the project).
 */
export function PreviewCanvas() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const paneRef = useRef<HTMLDivElement>(null);
  const [streaming, setStreaming] = useState(true);
  const [fps, setFps] = useState(0);
  const [frameIndex, setFrameIndex] = useState(0);
  // Whether at least one live frame has been decoded — gates enabling "set
  // reference" (a ref can't drive re-render, so mirror its presence into state).
  const [hasLiveFrame, setHasLiveFrame] = useState(false);
  // The engine's liveness state (#62), driven by the `engine-event` stream: badge
  // whether the pane is live, holding last-good, or render-stopped.
  const engineStatus = useDocumentStore((s) => s.engineStatus);
  // Mark which stream is active so the engine-event listener can ignore a
  // superseded stream's late events (#12/#13).
  const setActiveStreamId = useDocumentStore((s) => s.setActiveStreamId);

  // Compare (#60): mode / divider / captured reference — all UI-only state.
  const mode = useCompareStore((s) => s.mode);
  const orientation = useCompareStore((s) => s.orientation);
  const splitPos = useCompareStore((s) => s.splitPos);
  const reference = useCompareStore((s) => s.reference);
  const setReference = useCompareStore((s) => s.setReference);

  // The most recent decoded LIVE frame, retained so a compare-state change can
  // re-composite WITHOUT waiting for the next streamed frame (instant flip) and so
  // "set reference" can snapshot the current pixels. Held in a ref to stay out of
  // React's render cycle (the frame stream runs at display rate).
  const latestFrameRef = useRef<ImageData | null>(null);

  /** Paint the canvas from the latest live frame + current compare state. */
  const composite = useCallback(() => {
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    const live = latestFrameRef.current;
    if (!canvas || !ctx || !live) {
      return;
    }
    if (canvas.width !== live.width || canvas.height !== live.height) {
      canvas.width = live.width;
      canvas.height = live.height;
    }
    drawCompare(
      ctx,
      live,
      reference,
      mode,
      splitPos,
      orientation,
      canvas.width,
      canvas.height,
    );
  }, [mode, splitPos, orientation, reference]);

  // Keep the LATEST `composite` in a ref so the long-lived frame-stream closure
  // (whose effect only re-runs on `streaming`) always paints with the current
  // compare state, without re-subscribing the channel on every toggle.
  const compositeRef = useRef(composite);
  compositeRef.current = composite;

  // Re-composite whenever the compare state changes (instant flip / divider drag),
  // reusing the retained live frame — no engine round-trip, frames keep streaming.
  useEffect(() => {
    composite();
  }, [composite]);

  useEffect(() => {
    if (!streaming) {
      return;
    }
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) {
      return;
    }

    let cancelled = false;
    let framesThisWindow = 0;
    let windowStart = performance.now();

    // Correlate this stream's start with its cleanup stop, so an out-of-order
    // stop (StrictMode mount→unmount→mount, or rapid Start/Stop) can't kill a
    // newer stream — the backend ignores a stop whose id is no longer active.
    const streamId = crypto.randomUUID();
    // Become the active stream so the engine-event listener accepts THIS stream's
    // events and ignores a prior (superseded) stream's late ones (#12/#13).
    setActiveStreamId(streamId);

    const channel = new Channel<ArrayBuffer>();
    channel.onmessage = (message) => {
      if (cancelled) {
        return;
      }
      let frame;
      try {
        frame = parseFrame(toArrayBuffer(message));
      } catch {
        return; // drop malformed frames rather than tearing down the stream
      }

      // Retain the latest live frame (#60) as an ImageData, then composite the
      // current compare view. The ImageData wraps the parsed pixel buffer; the
      // reference (if any) was deep-copied at capture, so it is decoupled from
      // this per-frame buffer.
      if (canvas.width !== frame.width || canvas.height !== frame.height) {
        canvas.width = frame.width;
        canvas.height = frame.height;
      }
      latestFrameRef.current = new ImageData(frame.pixels, frame.width, frame.height);
      if (!cancelled) {
        setHasLiveFrame(true);
      }
      compositeRef.current();

      framesThisWindow += 1;
      const now = performance.now();
      const elapsed = now - windowStart;
      if (elapsed >= 500) {
        setFps(Math.round((framesThisWindow * 1000) / elapsed));
        setFrameIndex(frame.frameIndex);
        framesThisWindow = 0;
        windowStart = now;
      }
    };

    void invoke("start_preview_stream", {
      channel,
      streamId,
      width: PREVIEW_WIDTH,
      height: PREVIEW_HEIGHT,
    });

    // Kick a real render: the built-in passthrough shader over the built-in
    // test pattern (null paths). This replaces Phase 0's fake-gradient trigger;
    // until these land the stream shows a solid "waiting" frame.
    invoke("load_shader", { shaderPath: null }).catch((e) =>
      console.error("load_shader failed", e),
    );
    invoke("load_source", { sourcePath: null }).catch((e) =>
      console.error("load_source failed", e),
    );

    return () => {
      cancelled = true;
      // Only relinquish active status if a newer stream hasn't already claimed it
      // (guards the StrictMode mount→unmount→mount ordering, #12/#13).
      if (useDocumentStore.getState().activeStreamId === streamId) {
        setActiveStreamId(null);
      }
      void invoke("stop_preview_stream", { streamId });
    };
    // `composite` is intentionally omitted: re-subscribing the stream on a compare
    // toggle would tear down/recreate the channel. The frame handler calls the
    // latest `composite` via `compositeRef`; the dedicated effect above re-paints
    // on compare changes for the no-new-frame case.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [streaming]);

  // "Set reference": snapshot the current live frame as A. Inert (no frame yet) is
  // guarded by disabling the control; this is a no-op then.
  const handleSetReference = useCallback(() => {
    const live = latestFrameRef.current;
    if (live) {
      setReference(live);
    }
  }, [setReference]);

  // ---- Pixel inspector (#61) ----
  // The inspector reads a pixel ON DEMAND via the `inspect_pixel` command (a
  // render-thread readback) on hover/click — NEVER per frame, so the 60 fps stream
  // is untouched. We measure the canvas's displayed box so crosshairs land on the
  // right CSS spot over the object-fit:contain image.
  const inspectorEnabled = useInspectorStore((s) => s.enabled);
  const setHover = useInspectorStore((s) => s.setHover);
  const pin = useInspectorStore((s) => s.pin);
  const [canvasGeometry, setCanvasGeometry] = useState<CanvasGeometry | null>(null);

  // Track the canvas's displayed box so the overlay can place crosshairs. Measured
  // on mount + resize (ResizeObserver) since the pane flexes with the window.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) {
      return;
    }
    const measure = () => {
      const rect = canvas.getBoundingClientRect();
      setCanvasGeometry({
        boxWidth: rect.width,
        boxHeight: rect.height,
        canvasWidth: canvas.width,
        canvasHeight: canvas.height,
        // Offset within the `.preview__viewport` (the canvas's offset parent), so
        // the split divider aligns its line with the canvas's contain rect (#1).
        offsetLeft: canvas.offsetLeft,
        offsetTop: canvas.offsetTop,
      });
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(canvas);
    return () => observer.disconnect();
  }, []);

  // The last pane pixel a probe was issued for, to debounce: a fresh `inspect_pixel`
  // is only fired when the cursor moves to a different pane pixel (a per-move probe
  // would spam the render thread). A pending flag drops overlapping requests.
  const lastProbedRef = useRef<string | null>(null);
  const probeInFlightRef = useRef(false);

  /** Map a pointer/mouse event to the canvas pixel under it (object-fit:contain
   * aware). Accepts the common `clientX/clientY` shape so both pointer and click
   * events can call it. */
  const eventToCanvasPixel = useCallback(
    (e: { clientX: number; clientY: number }) => {
      const canvas = canvasRef.current;
      if (!canvas) {
        return null;
      }
      const rect = canvas.getBoundingClientRect();
      return domToCanvasPixel(
        e.clientX - rect.left,
        e.clientY - rect.top,
        rect.width,
        rect.height,
        canvas.width,
        canvas.height,
      );
    },
    [],
  );

  /** Probe one pane pixel via the render-thread readback, updating the hover. */
  const probePixel = useCallback(
    async (px: number, py: number): Promise<PixelSample | null> => {
      if (probeInFlightRef.current) {
        return null;
      }
      probeInFlightRef.current = true;
      try {
        const sample = await invoke<PixelSample>("inspect_pixel", { x: px, y: py });
        return sample;
      } catch {
        return null; // a stalled/torn-down render thread: drop this probe
      } finally {
        probeInFlightRef.current = false;
      }
    },
    [],
  );

  const handlePointerMove = useCallback(
    (e: React.PointerEvent<HTMLCanvasElement>) => {
      if (!inspectorEnabled) {
        return;
      }
      const pane = eventToCanvasPixel(e);
      if (!pane) {
        // In the contain margin (off the rendered image): clear the hover.
        setHover(null);
        lastProbedRef.current = null;
        return;
      }
      const key = `${pane.x},${pane.y}`;
      if (key === lastProbedRef.current) {
        return; // same pane pixel — debounce
      }
      lastProbedRef.current = key;
      void probePixel(pane.x, pane.y).then((sample) => {
        if (sample) {
          setHover({ pane, sample });
        }
      });
    },
    [inspectorEnabled, eventToCanvasPixel, probePixel, setHover],
  );

  const handlePointerLeave = useCallback(() => {
    setHover(null);
    lastProbedRef.current = null;
  }, [setHover]);

  // Click-to-pin: probe the clicked pixel and pin the result (persists while
  // panning the graph; cleared via the inspector controls).
  const handleClick = useCallback(
    (e: React.MouseEvent<HTMLCanvasElement>) => {
      if (!inspectorEnabled) {
        return;
      }
      const pane = eventToCanvasPixel(e);
      if (!pane) {
        return;
      }
      void probePixel(pane.x, pane.y).then((sample) => {
        if (sample && sample.inside) {
          pin(pane, sample);
        }
      });
    },
    [inspectorEnabled, eventToCanvasPixel, probePixel, pin],
  );

  return (
    <div className="preview__canvas-wrap">
      <div className="preview__viewport" ref={paneRef}>
        <canvas
          ref={canvasRef}
          className={`preview__canvas${inspectorEnabled ? " preview__canvas--inspecting" : ""}`}
          width={PREVIEW_WIDTH}
          height={PREVIEW_HEIGHT}
          onPointerMove={handlePointerMove}
          onPointerLeave={handlePointerLeave}
          onClick={handleClick}
        />
        {mode === "split" && reference && canvasGeometry ? (
          <SplitDivider
            canvasRef={canvasRef}
            geometry={canvasGeometry}
            orientation={orientation}
            pos={splitPos}
          />
        ) : null}
        <InspectorOverlay geometry={canvasGeometry} />
      </div>
      <CompareControls
        hasLiveFrame={hasLiveFrame}
        onSetReference={handleSetReference}
      />
      <InspectorControls />
      <div className="preview__stats">
        <button type="button" onClick={() => setStreaming((s) => !s)}>
          {streaming ? "Stop" : "Start"}
        </button>
        <span className="preview__stat">{streaming ? `${fps} fps` : "stopped"}</span>
        <span className="preview__stat">frame {frameIndex}</span>
        {streaming && engineStatus ? (
          <span
            className={`preview__status preview__status--${engineStatus}`}
            data-testid="preview-status"
          >
            {STATUS_LABEL[engineStatus]}
          </span>
        ) : null}
      </div>
    </div>
  );
}
