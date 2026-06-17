import { Channel, invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";

import { parseFrame, toArrayBuffer } from "./frame";

const PREVIEW_WIDTH = 512;
const PREVIEW_HEIGHT = 384;

/**
 * Hosts the preview `<canvas>` and drives the Rust → webview binary frame
 * stream (Architecture §F). The frames are the offscreen wgpu render read back
 * and downsampled to the pane: `load_shader` + `load_source` (null paths => the
 * built-in passthrough over a test pattern) kick the real render once the
 * stream is up. File pickers that pass real `.slang`/image paths arrive later.
 */
export function PreviewCanvas() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [streaming, setStreaming] = useState(true);
  const [fps, setFps] = useState(0);
  const [frameIndex, setFrameIndex] = useState(0);

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

      if (canvas.width !== frame.width || canvas.height !== frame.height) {
        canvas.width = frame.width;
        canvas.height = frame.height;
      }
      ctx.putImageData(new ImageData(frame.pixels, frame.width, frame.height), 0, 0);

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
  }, [streaming]);

  return (
    <div className="preview__canvas-wrap">
      <canvas
        ref={canvasRef}
        className="preview__canvas"
        width={PREVIEW_WIDTH}
        height={PREVIEW_HEIGHT}
      />
      <div className="preview__stats">
        <button type="button" onClick={() => setStreaming((s) => !s)}>
          {streaming ? "Stop" : "Start"}
        </button>
        <span className="preview__stat">{streaming ? `${fps} fps` : "stopped"}</span>
        <span className="preview__stat">frame {frameIndex}</span>
      </div>
    </div>
  );
}
