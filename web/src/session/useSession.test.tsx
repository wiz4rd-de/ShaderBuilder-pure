import { render, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Recovery } from "../bindings/Recovery";
import { useDocumentStore } from "../store/documentStore";
import { resetIdsForTest } from "../store/ids";
import { makeProject } from "../store/factories";
import { useConfirmStore } from "./confirmStore";
import { useSession, type SessionHookDeps } from "./useSession";

function store() {
  return useDocumentStore.getState();
}

function fakeDeps(over: Partial<SessionHookDeps> = {}): SessionHookDeps {
  return {
    setBackendDirty: vi.fn(async () => undefined),
    autosaveRecovery: vi.fn(async () => undefined),
    checkRecovery: vi.fn(async () => null),
    onCloseRequested: vi.fn(async () => () => undefined),
    closeWindow: vi.fn(async () => undefined),
    ...over,
  };
}

/**
 * An `onCloseRequested` seam that stashes the backend handler and exposes `fire()`
 * to drive a close-request synchronously (mirrors useEngineEvents.test.tsx). Used
 * to exercise the close guard's proceed/cancel wiring (F16).
 */
function makeCloseRequested() {
  let handler: (() => void) | null = null;
  const onCloseRequested = vi.fn<SessionHookDeps["onCloseRequested"]>((h) => {
    handler = h;
    return Promise.resolve(() => {
      handler = null;
    });
  });
  return { onCloseRequested, fire: () => handler?.() };
}

/** Auto-answer the next confirm prompt with `choice` (after it opens). */
function autoAnswer(choice: "confirm" | "discard" | "cancel"): void {
  const unsub = useConfirmStore.subscribe((s) => {
    if (s.prompt) {
      unsub();
      s.prompt.resolve(choice);
      useConfirmStore.setState({ prompt: null });
    }
  });
}

/** Let queued microtasks (the async close-guard chain) settle. */
async function flush(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

function Harness({ deps }: { deps: SessionHookDeps }) {
  useSession(deps);
  return null;
}

beforeEach(() => {
  resetIdsForTest();
  store().reset();
  useConfirmStore.setState({ prompt: null });
});

describe("useSession — dirty mirror", () => {
  it("pushes the initial dirty value and every transition to the backend", async () => {
    const deps = fakeDeps();
    render(<Harness deps={deps} />);
    await waitFor(() => expect(deps.setBackendDirty).toHaveBeenCalledWith(false));

    store().addNode("placeholder", { x: 0, y: 0 }); // -> dirty true
    await waitFor(() => expect(deps.setBackendDirty).toHaveBeenCalledWith(true));
  });
});

describe("useSession — recovery offer", () => {
  it("offers a restore and applies it on confirm", async () => {
    const recovery: Recovery = {
      project: makeProject("Recovered"),
      meta: { projectPath: null, name: "Recovered", savedAtMs: 1n },
    };
    const deps = fakeDeps({ checkRecovery: vi.fn(async () => recovery) });
    render(<Harness deps={deps} />);

    // The prompt opens; answer "confirm" (Restore).
    await waitFor(() => expect(useConfirmStore.getState().prompt).not.toBeNull());
    useConfirmStore.getState().answer("confirm");

    await waitFor(() => expect(store().project.name).toBe("Recovered"));
    // A recovered doc is unsaved.
    expect(store().dirty).toBe(true);
  });

  it("does not prompt when there is no recovery", async () => {
    const deps = fakeDeps({ checkRecovery: vi.fn(async () => null) });
    render(<Harness deps={deps} />);
    await waitFor(() => expect(deps.checkRecovery).toHaveBeenCalled());
    expect(useConfirmStore.getState().prompt).toBeNull();
  });
});

describe("useSession — close guard", () => {
  it("subscribes to the backend close-requested event", () => {
    const deps = fakeDeps();
    render(<Harness deps={deps} />);
    expect(deps.onCloseRequested).toHaveBeenCalled();
  });

  it("dirty + cancel: does NOT close and leaves the doc dirty", async () => {
    const { onCloseRequested, fire } = makeCloseRequested();
    const deps = fakeDeps({ onCloseRequested });
    render(<Harness deps={deps} />);
    await waitFor(() => expect(onCloseRequested).toHaveBeenCalled());

    store().addNode("placeholder", { x: 0, y: 0 }); // -> dirty
    expect(store().dirty).toBe(true);

    autoAnswer("cancel");
    fire();
    await flush();

    expect(deps.closeWindow).not.toHaveBeenCalled();
    expect(store().dirty).toBe(true); // still has unsaved work
  });

  it("dirty + discard: closes the window once", async () => {
    const { onCloseRequested, fire } = makeCloseRequested();
    const deps = fakeDeps({ onCloseRequested });
    render(<Harness deps={deps} />);
    await waitFor(() => expect(onCloseRequested).toHaveBeenCalled());

    store().addNode("placeholder", { x: 0, y: 0 });
    autoAnswer("discard");
    fire();
    await flush();

    expect(deps.closeWindow).toHaveBeenCalledOnce();
  });

  it("clean: proceeds and closes the window without prompting", async () => {
    const { onCloseRequested, fire } = makeCloseRequested();
    const deps = fakeDeps({ onCloseRequested });
    render(<Harness deps={deps} />);
    await waitFor(() => expect(onCloseRequested).toHaveBeenCalled());

    expect(store().dirty).toBe(false);
    let prompted = false;
    const unsub = useConfirmStore.subscribe((s) => {
      if (s.prompt) {
        prompted = true;
      }
    });
    fire();
    await flush();
    unsub();

    expect(prompted).toBe(false); // a clean doc skips the prompt
    expect(deps.closeWindow).toHaveBeenCalledOnce();
  });
});
