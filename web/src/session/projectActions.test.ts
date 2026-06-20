import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Project } from "../bindings/Project";
import type { RecentProject } from "../bindings/RecentProject";
import { useDocumentStore } from "../store/documentStore";
import { resetIdsForTest } from "../store/ids";
import { useConfirmStore } from "./confirmStore";
import {
  newProject,
  openProject,
  openRecent,
  save,
  saveAs,
  type SessionDeps,
} from "./projectActions";

function store() {
  return useDocumentStore.getState();
}

/** A SessionDeps whose IO seams are vi.fns with sensible defaults. */
function fakeDeps(over: Partial<SessionDeps> = {}): SessionDeps {
  return {
    saveProject: vi.fn(async () => undefined),
    loadProject: vi.fn(async () => store().project),
    pickOpenPath: vi.fn(async () => null),
    pickSavePath: vi.fn(async () => null),
    pushRecent: vi.fn(async () => [] as RecentProject[]),
    loadRecents: vi.fn(async () => [] as RecentProject[]),
    clearRecovery: vi.fn(async () => undefined),
    autosaveRecovery: vi.fn(async () => undefined),
    now: () => "2026-06-18T00:00:00.000Z",
    ...over,
  };
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

beforeEach(() => {
  resetIdsForTest();
  store().reset();
  useConfirmStore.setState({ prompt: null });
});

describe("save / saveAs", () => {
  it("save with no path falls through to Save-As (prompts for a path)", async () => {
    const deps = fakeDeps({ pickSavePath: vi.fn(async () => "/tmp/a.json") });
    store().addNode("placeholder", { x: 0, y: 0 }); // make it dirty
    expect(store().dirty).toBe(true);

    const ok = await save(deps);

    expect(ok).toBe(true);
    expect(deps.pickSavePath).toHaveBeenCalledOnce();
    expect(deps.saveProject).toHaveBeenCalledOnce();
    expect(store().dirty).toBe(false);
    expect(store().currentProjectPath).toBe("/tmp/a.json");
  });

  it("save with an existing path overwrites without prompting", async () => {
    const deps = fakeDeps();
    useDocumentStore.setState({ currentProjectPath: "/tmp/known.json" });
    store().addNode("placeholder", { x: 0, y: 0 });

    const ok = await save(deps);

    expect(ok).toBe(true);
    expect(deps.pickSavePath).not.toHaveBeenCalled();
    const [path] = (deps.saveProject as ReturnType<typeof vi.fn>).mock.calls[0]!;
    expect(path).toBe("/tmp/known.json");
  });

  it("stamps metadata.modifiedAt (and createdAt) on save", async () => {
    const deps = fakeDeps({ pickSavePath: vi.fn(async () => "/tmp/a.json") });
    await save(deps);
    const written = (deps.saveProject as ReturnType<typeof vi.fn>).mock
      .calls[0]![1] as Project;
    expect(written.metadata.modifiedAt).toBe("2026-06-18T00:00:00.000Z");
    expect(written.metadata.createdAt).toBe("2026-06-18T00:00:00.000Z");
  });

  it("records the saved file in recents and clears recovery", async () => {
    const deps = fakeDeps({ pickSavePath: vi.fn(async () => "/tmp/a.json") });
    await save(deps);
    expect(deps.pushRecent).toHaveBeenCalledWith({
      path: "/tmp/a.json",
      name: store().project.name,
    });
    expect(deps.clearRecovery).toHaveBeenCalled();
  });

  it("saveAs cancelled leaves the document dirty and unsaved", async () => {
    const deps = fakeDeps({ pickSavePath: vi.fn(async () => null) });
    store().addNode("placeholder", { x: 0, y: 0 });
    const ok = await saveAs(deps);
    expect(ok).toBe(false);
    expect(store().dirty).toBe(true);
    expect(deps.saveProject).not.toHaveBeenCalled();
  });

  it("preserves an edit made DURING the in-flight save (F10)", async () => {
    // The save serializes a pre-await snapshot, but a node added while the write is
    // in flight must NOT be clobbered by that stale snapshot when the store commits.
    let editId = "";
    const deps = fakeDeps({
      pickSavePath: vi.fn(async () => "/tmp/a.json"),
      saveProject: vi.fn(async () => {
        // Simulate the user editing mid-write (between snapshot and commit).
        editId = store().addNode("placeholder", { x: 10, y: 10 });
      }),
    });

    const ok = await save(deps);

    expect(ok).toBe(true);
    // The mid-save edit survives in the committed document.
    expect(store().activeGraph().nodes.some((n) => n.id === editId)).toBe(true);
    // The save path was still recorded.
    expect(store().currentProjectPath).toBe("/tmp/a.json");
    // The committed project carries the stamped metadata.
    expect(store().project.metadata.modifiedAt).toBe("2026-06-18T00:00:00.000Z");
  });

  it("a save IO failure returns false and keeps the document dirty", async () => {
    const deps = fakeDeps({
      pickSavePath: vi.fn(async () => "/tmp/a.json"),
      saveProject: vi.fn(async () => {
        throw { kind: "io", message: "disk full" };
      }),
    });
    store().addNode("placeholder", { x: 0, y: 0 });
    const ok = await save(deps);
    expect(ok).toBe(false);
    expect(store().dirty).toBe(true);
  });
});

describe("new — confirm-discard guard", () => {
  it("cancel ABORTS the new (document is preserved)", async () => {
    const deps = fakeDeps();
    const id = store().addNode("placeholder", { x: 0, y: 0 });
    autoAnswer("cancel");

    await newProject(deps);

    // Still the same dirty document — not reset.
    expect(store().dirty).toBe(true);
    expect(store().activeGraph().nodes.some((n) => n.id === id)).toBe(true);
  });

  it("discard proceeds and resets to a fresh untitled project", async () => {
    const deps = fakeDeps();
    store().addNode("placeholder", { x: 0, y: 0 });
    autoAnswer("discard");

    await newProject(deps);

    expect(store().dirty).toBe(false);
    expect(store().currentProjectPath).toBeNull();
    expect(store().activeGraph().nodes).toHaveLength(0);
    expect(deps.clearRecovery).toHaveBeenCalled();
  });

  it("save-then-proceed writes the doc before resetting", async () => {
    const deps = fakeDeps({ pickSavePath: vi.fn(async () => "/tmp/a.json") });
    store().addNode("placeholder", { x: 0, y: 0 });
    autoAnswer("confirm");

    await newProject(deps);

    expect(deps.saveProject).toHaveBeenCalledOnce();
    expect(store().activeGraph().nodes).toHaveLength(0);
  });

  it("save-then-proceed ABORTS if the save was cancelled", async () => {
    const deps = fakeDeps({ pickSavePath: vi.fn(async () => null) });
    const id = store().addNode("placeholder", { x: 0, y: 0 });
    autoAnswer("confirm");

    await newProject(deps);

    // Save-As cancelled -> guard returns false -> new is aborted.
    expect(store().activeGraph().nodes.some((n) => n.id === id)).toBe(true);
  });

  it("a clean document does not prompt at all", async () => {
    const deps = fakeDeps();
    let asked = false;
    const unsub = useConfirmStore.subscribe((s) => {
      if (s.prompt) {
        asked = true;
      }
    });
    await newProject(deps);
    unsub();
    expect(asked).toBe(false);
    expect(store().activeGraph().nodes).toHaveLength(0);
  });
});

describe("open / open recent", () => {
  it("open loads the picked file and records it in recents", async () => {
    const loaded: Project = { ...store().project, name: "Loaded" };
    const deps = fakeDeps({
      pickOpenPath: vi.fn(async () => "/tmp/loaded.json"),
      loadProject: vi.fn(async () => loaded),
    });

    await openProject(deps);

    expect(store().project.name).toBe("Loaded");
    expect(store().currentProjectPath).toBe("/tmp/loaded.json");
    expect(store().dirty).toBe(false);
    expect(deps.pushRecent).toHaveBeenCalledWith({
      path: "/tmp/loaded.json",
      name: "Loaded",
    });
  });

  it("open cancelled is a no-op", async () => {
    const deps = fakeDeps({ pickOpenPath: vi.fn(async () => null) });
    await openProject(deps);
    expect(deps.loadProject).not.toHaveBeenCalled();
  });

  it("openRecent of a missing/malformed file fails gracefully (no throw, no load)", async () => {
    const deps = fakeDeps({
      loadProject: vi.fn(async () => {
        throw { kind: "io", message: "NotFound" };
      }),
    });
    // Should not reject.
    await expect(openRecent("/tmp/gone.json", deps)).resolves.toBeUndefined();
    // The current document is untouched.
    expect(store().currentProjectPath).toBeNull();
  });

  it("openRecent guards unsaved edits and aborts on cancel", async () => {
    const deps = fakeDeps({ loadProject: vi.fn(async () => store().project) });
    store().addNode("placeholder", { x: 0, y: 0 });
    autoAnswer("cancel");
    await openRecent("/tmp/x.json", deps);
    expect(deps.loadProject).not.toHaveBeenCalled();
  });
});
