import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Project } from "../bindings/Project";
import { makeProject } from "../store/factories";
import { useDocumentStore } from "../store/documentStore";
import { useOnboardingStore } from "./onboardingStore";
import type { StartDeps } from "./startActions";
import { startExample, startImport, startNew, startOpen } from "./startActions";

function exampleProject(): Project {
  return { ...makeProject("CRT Scanlines + Curvature") };
}

/** A StartDeps with all seams stubbed; tests override the ones they exercise. */
function deps(overrides: Partial<StartDeps> = {}): StartDeps {
  return {
    newProject: vi.fn(async () => undefined),
    openProject: vi.fn(async () => undefined),
    loadExampleProject: vi.fn(async () => exampleProject()),
    importPreset: vi.fn(async () => exampleProject()),
    pickPresetPath: vi.fn(async () => null),
    ...overrides,
  };
}

beforeEach(() => {
  useOnboardingStore.getState().reset();
  useDocumentStore.getState().reset();
});

describe("startActions", () => {
  it("starts false and New enters the editor", async () => {
    expect(useOnboardingStore.getState().started).toBe(false);
    const d = deps();
    await startNew(d);
    expect(d.newProject).toHaveBeenCalledOnce();
    expect(useOnboardingStore.getState().started).toBe(true);
  });

  it("Open example loads the bundled project as an untitled doc and enters", async () => {
    const d = deps();
    await startExample(d);
    expect(d.loadExampleProject).toHaveBeenCalledOnce();
    expect(useOnboardingStore.getState().started).toBe(true);
    const store = useDocumentStore.getState();
    expect(store.project.name).toBe("CRT Scanlines + Curvature");
    // Loaded as untitled (a starting point), so there is no associated path.
    expect(store.currentProjectPath).toBeNull();
  });

  it("Open example stays on the start screen and toasts when loading fails", async () => {
    const d = deps({
      loadExampleProject: vi.fn(async () => {
        throw new Error("boom");
      }),
    });
    await startExample(d);
    expect(useOnboardingStore.getState().started).toBe(false);
  });

  it("Import preset that is cancelled keeps the start screen", async () => {
    const d = deps({ pickPresetPath: vi.fn(async () => null) });
    await startImport(d);
    expect(d.importPreset).not.toHaveBeenCalled();
    expect(useOnboardingStore.getState().started).toBe(false);
  });

  it("Import preset loads the imported project and enters", async () => {
    const d = deps({
      pickPresetPath: vi.fn(async () => "/some/preset.slangp"),
      importPreset: vi.fn(async () => exampleProject()),
    });
    await startImport(d);
    expect(d.importPreset).toHaveBeenCalledWith("/some/preset.slangp");
    expect(useOnboardingStore.getState().started).toBe(true);
  });

  it("Open that is cancelled (no path change) keeps the start screen", async () => {
    // openProject is a no-op stub here → currentProjectPath unchanged → no enter.
    const d = deps({ openProject: vi.fn(async () => undefined) });
    await startOpen(d);
    expect(useOnboardingStore.getState().started).toBe(false);
  });

  it("Open that loads a project (path set) enters the editor", async () => {
    const d = deps({
      openProject: vi.fn(async () => {
        useDocumentStore.setState({ currentProjectPath: "/loaded.json" });
      }),
    });
    await startOpen(d);
    expect(useOnboardingStore.getState().started).toBe(true);
  });
});
