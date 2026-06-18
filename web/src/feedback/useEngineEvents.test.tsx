import { render } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { EngineEvent } from "../bindings/EngineEvent";
import { useDocumentStore } from "../store/documentStore";
import { resetIdsForTest } from "../store/ids";
import { useToastStore } from "./toastStore";
import { useEngineEvents, type EngineEventSubscribe } from "./useEngineEvents";

function doc() {
  return useDocumentStore.getState();
}
function toasts() {
  return useToastStore.getState();
}

/**
 * A test host: mounts the hook with an injected subscriber that exposes the
 * captured handler so the test can fire engine events synchronously.
 */
function Harness({ subscribe }: { subscribe: EngineEventSubscribe }): React.JSX.Element {
  useEngineEvents({ subscribe });
  return <div />;
}

/** Build an injectable subscriber + a `fire` to push events into the hook. */
function makeSubscriber(): {
  subscribe: EngineEventSubscribe;
  fire: (event: EngineEvent) => void;
} {
  let handler: ((event: EngineEvent) => void) | null = null;
  const subscribe: EngineEventSubscribe = (h) => {
    handler = h;
    return Promise.resolve(() => {
      handler = null;
    });
  };
  return { subscribe, fire: (event) => handler?.(event) };
}

beforeEach(() => {
  vi.useRealTimers();
  resetIdsForTest();
  doc().reset();
  // The active stream id is stream-lifecycle state (not document state, so reset()
  // leaves it) — clear it between tests so a prior test's stream doesn't filter the
  // next test's events (#12/#13).
  doc().setActiveStreamId(null);
  toasts().clear();
});

describe("useEngineEvents", () => {
  it("routes a status event into the store engineStatus", async () => {
    const { subscribe, fire } = makeSubscriber();
    render(<Harness subscribe={subscribe} />);
    await Promise.resolve();

    fire({ kind: "status", streamId: "s1", status: "lastGood" });
    expect(doc().engineStatus).toBe("lastGood");

    fire({ kind: "status", streamId: "s1", status: "live" });
    expect(doc().engineStatus).toBe("live");
  });

  it("raises a toast on a stopped status", async () => {
    const { subscribe, fire } = makeSubscriber();
    render(<Harness subscribe={subscribe} />);
    await Promise.resolve();

    fire({ kind: "status", streamId: "s1", status: "stopped" });
    expect(doc().engineStatus).toBe("stopped");
    expect(toasts().toasts.some((t) => t.message.includes("render stopped"))).toBe(true);
  });

  it("ignores events from a superseded stream (no toast, no status clobber) (#12/#13)", async () => {
    const { subscribe, fire } = makeSubscriber();
    render(<Harness subscribe={subscribe} />);
    await Promise.resolve();

    // The active stream is "new"; the old "stale" stream is superseded.
    doc().setActiveStreamId("new");
    fire({ kind: "status", streamId: "new", status: "live" });
    expect(doc().engineStatus).toBe("live");

    // A late Stopped from the superseded stream must NOT toast nor clobber Live.
    fire({ kind: "status", streamId: "stale", status: "stopped" });
    expect(doc().engineStatus).toBe("live");
    expect(toasts().toasts.some((t) => t.message.includes("render stopped"))).toBe(false);

    // The active stream's own events still win.
    fire({ kind: "status", streamId: "new", status: "lastGood" });
    expect(doc().engineStatus).toBe("lastGood");
  });

  it("drops a superseded stream's error from the problems panel (#12/#13)", async () => {
    const { subscribe, fire } = makeSubscriber();
    render(<Harness subscribe={subscribe} />);
    await Promise.resolve();

    doc().setActiveStreamId("new");
    fire({
      kind: "error",
      streamId: "stale",
      error: {
        severity: "error",
        code: "deviceLost",
        message: "old thread died",
        passId: null,
        nodeId: null,
      },
    });
    expect(doc().engineProblems).toHaveLength(0);
    expect(toasts().toasts).toHaveLength(0);
  });

  it("synthesizes an engine problem + a toast from an error event", async () => {
    const { subscribe, fire } = makeSubscriber();
    render(<Harness subscribe={subscribe} />);
    await Promise.resolve();

    fire({
      kind: "error",
      streamId: "s1",
      error: {
        severity: "error",
        code: "slangCompile",
        message: "syntax error",
        passId: "pass-1",
        nodeId: null,
      },
    });

    expect(doc().engineProblems).toHaveLength(1);
    expect(doc().engineProblems[0]).toMatchObject({
      passId: "pass-1",
      origin: "engine",
      diagnostic: { code: "slangCompile", message: "syntax error" },
    });
    const toast = toasts().toasts[0]!;
    expect(toast.severity).toBe("error");
    expect(toast.message).toContain("Shader compile failed");
    expect(toast.message).toContain("syntax error");
  });

  it("unsubscribes on unmount", async () => {
    const off = vi.fn();
    const subscribe: EngineEventSubscribe = () => Promise.resolve(off);
    const { unmount } = render(<Harness subscribe={subscribe} />);
    await Promise.resolve();
    unmount();
    expect(off).toHaveBeenCalledTimes(1);
  });
});
