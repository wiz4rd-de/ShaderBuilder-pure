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
  toasts().clear();
});

describe("useEngineEvents", () => {
  it("routes a status event into the store engineStatus", async () => {
    const { subscribe, fire } = makeSubscriber();
    render(<Harness subscribe={subscribe} />);
    await Promise.resolve();

    fire({ kind: "status", status: "lastGood" });
    expect(doc().engineStatus).toBe("lastGood");

    fire({ kind: "status", status: "live" });
    expect(doc().engineStatus).toBe("live");
  });

  it("raises a toast on a stopped status", async () => {
    const { subscribe, fire } = makeSubscriber();
    render(<Harness subscribe={subscribe} />);
    await Promise.resolve();

    fire({ kind: "status", status: "stopped" });
    expect(doc().engineStatus).toBe("stopped");
    expect(toasts().toasts.some((t) => t.message.includes("render stopped"))).toBe(true);
  });

  it("synthesizes an engine problem + a toast from an error event", async () => {
    const { subscribe, fire } = makeSubscriber();
    render(<Harness subscribe={subscribe} />);
    await Promise.resolve();

    fire({
      kind: "error",
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
