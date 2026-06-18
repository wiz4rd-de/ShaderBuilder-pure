import { describe, expect, it, vi } from "vitest";

import type { CompileGraphResult } from "../bindings/CompileGraphResult";
import type { ExportResult } from "../bindings/ExportResult";
import type { Graph } from "../bindings/Graph";
import type { Pass } from "../bindings/Pass";
import type { PassSettings } from "../bindings/PassSettings";
import type { Project } from "../bindings/Project";
import { EMPTY_PROJECT } from "../model";
import type { InvokeCompile } from "./compileLoop";
import { exportProject } from "./exportProject";

function settings(): PassSettings {
  return {
    scaleX: { scaleType: null, scale: null },
    scaleY: { scaleType: null, scale: null },
    filterLinear: null,
    wrapMode: null,
    mipmapInput: null,
    floatFramebuffer: null,
    srgbFramebuffer: null,
    alias: null,
    frameCountMod: null,
  };
}

function graphPass(id: string): Pass {
  return {
    id,
    name: id,
    source: {
      kind: "graph",
      graph: {
        nodes: [{ id: "out", kind: "output", position: { x: 0, y: 0 }, data: {} }],
        edges: [],
      } as Graph,
    },
    parameters: [],
    references: [],
    settings: settings(),
  };
}

function project(passes: Pass[]): Project {
  return { ...EMPTY_PROJECT, passes };
}

const result: ExportResult = {
  presetPath: "/out/preset.slangp",
  passFiles: ["pass0.slang"],
  textureFiles: [],
  warnings: [],
};

describe("exportProject", () => {
  it("substitutes the generated slang and exports the all-whole-pass project", async () => {
    const compile = vi.fn<InvokeCompile>(
      async () => ({ source: "// generated", diagnostics: { items: [] } }) as CompileGraphResult,
    );
    const exportPreset = vi.fn(async (_project: Project, _destDir: string) => result);

    const outcome = await exportProject(project([graphPass("g")]), "/out", {
      invokeCompile: compile,
      exportPreset,
    });

    expect(outcome.kind).toBe("ok");
    // The project handed to export_preset has the graph pass replaced by whole-pass.
    const exported = exportPreset.mock.calls[0]![0] as Project;
    expect(exported.passes[0]!.source.kind).toBe("wholePassCode");
    if (exported.passes[0]!.source.kind === "wholePassCode") {
      expect(exported.passes[0]!.source.source).toBe("// generated");
    }
  });

  it("refuses to export an invalid pipeline (a pass that did not compile)", async () => {
    const compile = vi.fn<InvokeCompile>(
      async () => ({ source: null, diagnostics: { items: [] } }) as CompileGraphResult,
    );
    const exportPreset = vi.fn(async () => result);

    const outcome = await exportProject(project([graphPass("bad")]), "/out", {
      invokeCompile: compile,
      exportPreset,
    });

    expect(outcome.kind).toBe("notRenderable");
    if (outcome.kind === "notRenderable") {
      expect(outcome.uncompiledPassIds).toEqual(["bad"]);
    }
    expect(exportPreset).not.toHaveBeenCalled();
  });

  it("surfaces a thrown ExportError as a typed error outcome", async () => {
    const compile = vi.fn<InvokeCompile>(
      async () => ({ source: "// gen", diagnostics: { items: [] } }) as CompileGraphResult,
    );
    const exportPreset = vi.fn(async () => {
      throw { kind: "io", message: "disk full" };
    });

    const outcome = await exportProject(project([graphPass("g")]), "/out", {
      invokeCompile: compile,
      exportPreset,
    });

    expect(outcome.kind).toBe("error");
    if (outcome.kind === "error") {
      expect(outcome.error).toEqual({ kind: "io", message: "disk full" });
    }
  });
});
