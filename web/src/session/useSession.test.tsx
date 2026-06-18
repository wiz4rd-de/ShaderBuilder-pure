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
});
