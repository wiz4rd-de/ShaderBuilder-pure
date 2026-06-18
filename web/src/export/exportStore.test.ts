import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ExportResult } from "../bindings/ExportResult";
import type { ExportValidation } from "../bindings/ExportValidation";
import type { ExportProjectOutcome } from "../compile/exportProject";
import { useDocumentStore } from "../store/documentStore";
import {
  defaultBundleName,
  joinDest,
  useExportStore,
  type ExportFlowDeps,
} from "./exportStore";

const VALID: ExportValidation = { blockers: [] };
const RESULT: ExportResult = {
  presetPath: "/dest/MyPreset/preset.slangp",
  passFiles: ["a.slang"],
  textureFiles: [],
  warnings: [],
};

function deps(over: Partial<ExportFlowDeps> = {}): ExportFlowDeps {
  return {
    validateExport: vi.fn(async () => VALID),
    pickExportDir: vi.fn(async () => "/dest"),
    revealPath: vi.fn(async () => undefined),
    exportProject: vi.fn(
      async (): Promise<ExportProjectOutcome> => ({ kind: "ok", result: RESULT }),
    ),
    ...over,
  };
}

beforeEach(() => {
  useExportStore.getState().closeDialog();
  useDocumentStore.getState().reset();
});

describe("defaultBundleName", () => {
  it("sanitizes the project name into a safe folder name", () => {
    expect(defaultBundleName("My Cool CRT!")).toBe("My_Cool_CRT");
    expect(defaultBundleName("   ")).toBe("preset");
  });
});

describe("joinDest", () => {
  it("joins with the platform separator", () => {
    expect(joinDest("/a/b", "c")).toBe("/a/b/c");
    expect(joinDest("/a/b/", "c")).toBe("/a/b/c");
    expect(joinDest("C:\\out", "c")).toBe("C:\\out\\c");
  });
});

describe("export store flow", () => {
  it("loads validation when opened and seeds the bundle name", async () => {
    useDocumentStore.setState((s) => ({ project: { ...s.project, name: "Demo Shader" } }));
    const d = deps();
    await useExportStore.getState().openDialog(d);
    const s = useExportStore.getState();
    expect(s.open).toBe(true);
    expect(s.bundleName).toBe("Demo_Shader");
    expect(s.validation).toEqual(VALID);
    expect(d.validateExport).toHaveBeenCalledOnce();
  });

  it("surfaces validation blockers (the gate disables export)", async () => {
    const blocked: ExportValidation = {
      blockers: [{ kind: "uncompiledGraphPass", passId: "g", passName: "G" }],
    };
    await useExportStore.getState().openDialog(deps({ validateExport: vi.fn(async () => blocked) }));
    expect(useExportStore.getState().validation).toEqual(blocked);
  });

  it("does not run export until a destination is chosen", async () => {
    const d = deps();
    await useExportStore.getState().openDialog(d);
    // No destDir yet → runExport is a no-op.
    await useExportStore.getState().runExport(d);
    expect(d.exportProject).not.toHaveBeenCalled();
    expect(useExportStore.getState().phase).toBe("form");
  });

  it("routes a successful export to the done phase with the result", async () => {
    const d = deps();
    await useExportStore.getState().openDialog(d);
    await useExportStore.getState().chooseDestination(d);
    useExportStore.getState().setBundleName("MyPreset");
    await useExportStore.getState().runExport(d);
    const s = useExportStore.getState();
    expect(s.phase).toBe("done");
    expect(s.result).toEqual(RESULT);
    // The full dest path = chosen dir + bundle name.
    expect(d.exportProject).toHaveBeenCalledWith(
      expect.any(Object),
      "/dest/MyPreset",
      undefined,
    );
  });

  it("routes a notRenderable outcome to a non-fatal error", async () => {
    const d = deps({
      exportProject: vi.fn(
        async (): Promise<ExportProjectOutcome> => ({
          kind: "notRenderable",
          uncompiledPassIds: ["bad"],
        }),
      ),
    });
    await useExportStore.getState().openDialog(d);
    await useExportStore.getState().chooseDestination(d);
    await useExportStore.getState().runExport(d);
    const s = useExportStore.getState();
    expect(s.phase).toBe("error");
    expect(s.errorMessage).toContain("bad");
  });

  it("routes a thrown io error to a non-fatal error message", async () => {
    const d = deps({
      exportProject: vi.fn(
        async (): Promise<ExportProjectOutcome> => ({
          kind: "error",
          error: { kind: "io", message: "permission denied" },
        }),
      ),
    });
    await useExportStore.getState().openDialog(d);
    await useExportStore.getState().chooseDestination(d);
    await useExportStore.getState().runExport(d);
    const s = useExportStore.getState();
    expect(s.phase).toBe("error");
    expect(s.errorMessage).toContain("permission denied");
  });

  it("reveals the written bundle, degrading to a note when the opener fails", async () => {
    const reveal = vi.fn(async () => {
      throw new Error("no opener");
    });
    const d = deps({ revealPath: reveal });
    await useExportStore.getState().openDialog(d);
    await useExportStore.getState().chooseDestination(d);
    await useExportStore.getState().runExport(d);
    await useExportStore.getState().reveal(d);
    expect(reveal).toHaveBeenCalledWith(RESULT.presetPath);
    expect(useExportStore.getState().errorMessage).toContain(RESULT.presetPath);
  });
});
