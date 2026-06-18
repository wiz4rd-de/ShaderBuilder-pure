// The engine-event listener hook (#62) — ingests the render thread's typed
// `engine-event` stream (status transitions + render/compile errors) and routes
// it into the editor's error surface:
//
//  * STATUS  → store.engineStatus (the preview pane badges live / last-good /
//    stopped). A `stopped` status also raises a toast so a dead render thread is
//    visible, not a silently-frozen pane.
//  * ERROR   → store.engineProblems (a pass/node-tagged diagnostic the Problems
//    panel renders) AND a non-blocking toast (so a transient render/IO/compile
//    failure is seen even when the panel is on another tab).
//
// The render engine never tears down on a bad chain (last-good is implicit), so
// this only SURFACES failures — the app stays usable throughout (no crash, no
// blank window for a render/compile/IO error).
//
// PURITY: the event subscription is injectable (`subscribe`) so the hook unit-tests
// with no Tauri runtime (the default uses the real `listen`).
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useEffect } from "react";

import type { EngineErrorEvent } from "../bindings/EngineErrorEvent";
import type { EngineEvent } from "../bindings/EngineEvent";
import { useDocumentStore } from "../store/documentStore";
import { useToastStore } from "./toastStore";

/** A short human label for the toast, derived from the error's stable code. */
function toastTitle(error: EngineErrorEvent): string {
  switch (error.code) {
    case "slangCompile":
      return "Shader compile failed";
    case "deviceLost":
      return "GPU device lost";
    case "noAdapter":
      return "No GPU available";
    case "renderFailed":
      return "Render failed";
    case "readback":
      return "Frame readback failed";
    case "fboAlloc":
      return "Framebuffer allocation failed";
    default:
      return "Render error";
  }
}

/** The injected event subscriber (tests pass a fake; default is the real Tauri one). */
export type EngineEventSubscribe = (
  handler: (event: EngineEvent) => void,
) => Promise<UnlistenFn>;

const defaultSubscribe: EngineEventSubscribe = (handler) =>
  listen<EngineEvent>("engine-event", (e) => handler(e.payload));

export interface UseEngineEventsOptions {
  subscribe?: EngineEventSubscribe;
}

/**
 * Subscribe to the engine's `engine-event` stream for the host component's
 * lifetime, routing status into the store and errors into the store + a toast.
 */
export function useEngineEvents(options: UseEngineEventsOptions = {}): void {
  const { subscribe = defaultSubscribe } = options;

  useEffect(() => {
    let unlisten: UnlistenFn | null = null;
    let disposed = false;

    const handle = (event: EngineEvent): void => {
      const { setEngineStatus, pushEngineProblem, activeStreamId } =
        useDocumentStore.getState();
      const { push: pushToast } = useToastStore.getState();
      // Ignore events from a superseded stream (#12/#13): a stopped/remounted old
      // render thread's late status must not toast nor clobber the live stream's
      // status. Only the currently-active stream's events are surfaced.
      if (activeStreamId !== null && event.streamId !== activeStreamId) {
        return;
      }
      if (event.kind === "status") {
        setEngineStatus(event.status);
        if (event.status === "stopped") {
          pushToast("warning", "Preview render stopped.");
        }
        return;
      }
      // An error event: synthesize a pass/node-tagged problem + a non-blocking toast.
      const { error } = event;
      pushEngineProblem(error);
      const severity = error.severity === "warning" ? "warning" : "error";
      pushToast(severity, `${toastTitle(error)}: ${error.message}`);
    };

    void subscribe(handle).then((off) => {
      if (disposed) {
        off();
      } else {
        unlisten = off;
      }
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [subscribe]);
}
