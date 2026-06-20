// Throttled `set_parameter` IPC (#53). Dragging a slider fires a value per
// pointermove (dozens/sec); the engine updates the parameter UBO directly with NO
// recompile, but we still must not flood the IPC bridge. So each set is coalesced
// per animation frame: only the LATEST value per name within a frame is sent, and
// a trailing send guarantees the final resting value lands.
//
// The engine no-ops unknown names and clamps to [min,max], so a stale/out-of-range
// value is harmless — but wasteful, hence the throttle. This module owns no React
// state; it is a thin scheduler over `invoke("set_parameter", {name, value})`.
import { invoke } from "@tauri-apps/api/core";

/** Send one parameter value to the engine (live UBO update, no recompile). */
export function sendParameter(name: string, value: number): void {
  void invoke("set_parameter", { name, value }).catch((err) =>
    // The engine ignores unknown names; a transport error is non-fatal here.
    console.error(`set_parameter(${name}) failed`, err),
  );
}

/**
 * A per-name rAF-coalescing sender. Calling `set(name, value)` many times within a
 * frame collapses to a SINGLE `set_parameter` carrying the latest value once the
 * frame fires. Always sends the trailing value so a slider's final position lands.
 */
export interface ThrottledParameterSender {
  /** Queue the latest value for `name`; flushes on the next animation frame. */
  set: (name: string, value: number) => void;
  /** Flush any pending values synchronously (e.g. on commit / unmount). */
  flush: () => void;
  /** Cancel a pending frame WITHOUT sending (teardown). */
  cancel: () => void;
}

/** rAF if available (browser/jsdom), else a setTimeout fallback (tests/SSR). */
function schedule(cb: () => void): () => void {
  if (typeof requestAnimationFrame === "function") {
    const id = requestAnimationFrame(() => cb());
    return () => cancelAnimationFrame(id);
  }
  const id = setTimeout(cb, 16);
  return () => clearTimeout(id);
}

/**
 * Build a throttled sender. `send` defaults to the live IPC call but is injectable
 * for tests (so they assert the coalescing without a Tauri runtime).
 */
export function createParameterSender(
  send: (name: string, value: number) => void = sendParameter,
): ThrottledParameterSender {
  const pending = new Map<string, number>();
  // Whether a frame is queued. A separate flag (not the canceller) is the source
  // of truth so a SYNCHRONOUS scheduler (the setTimeout fallback never is, but a
  // test stub may call the callback inline) cannot strand a stale canceller and
  // block future frames.
  let scheduled = false;
  let cancelFrame: (() => void) | null = null;

  const flush = (): void => {
    scheduled = false;
    cancelFrame = null;
    if (pending.size === 0) {
      return;
    }
    const batch = Array.from(pending.entries());
    pending.clear();
    for (const [name, value] of batch) {
      send(name, value);
    }
  };

  return {
    set(name, value) {
      pending.set(name, value);
      if (!scheduled) {
        scheduled = true;
        const canceller = schedule(flush);
        // If `schedule` ran `flush` inline (a synchronous stub), `scheduled` is
        // already false again — don't record a stale canceller.
        if (scheduled) {
          cancelFrame = canceller;
        }
      }
    },
    flush,
    cancel() {
      if (cancelFrame) {
        cancelFrame();
      }
      cancelFrame = null;
      scheduled = false;
      pending.clear();
    },
  };
}
