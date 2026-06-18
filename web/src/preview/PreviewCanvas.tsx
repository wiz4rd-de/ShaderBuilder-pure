import { Channel, invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";

import type { EngineStatus } from "../bindings/EngineStatus";
import { useDocumentStore } from "../store/documentStore";
import { CompareControls } from "./CompareControls";
import { drawCompare } from "./compareCompositor";
import { useCompareStore } from "./compareStore";
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

  return (
    <div className="preview__canvas-wrap">
      <div className="preview__viewport" ref={paneRef}>
        <canvas
          ref={canvasRef}
          className="preview__canvas"
          width={PREVIEW_WIDTH}
          height={PREVIEW_HEIGHT}
        />
        {mode === "split" && reference ? (
          <SplitDivider
            paneRef={paneRef}
            orientation={orientation}
            pos={splitPos}
          />
        ) : null}
      </div>
      <CompareControls
        hasLiveFrame={hasLiveFrame}
        onSetReference={handleSetReference}
      />
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
