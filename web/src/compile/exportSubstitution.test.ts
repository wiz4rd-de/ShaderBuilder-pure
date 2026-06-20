import { describe, expect, it } from "vitest";

import type { Graph } from "../bindings/Graph";
import type { Pass } from "../bindings/Pass";
import type { PassSettings } from "../bindings/PassSettings";
import type { Project } from "../bindings/Project";
import { EMPTY_PROJECT } from "../model";
import { sourcesFromCompile, substituteGraphPasses } from "./exportSubstitution";

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
    source: { kind: "graph", graph: { nodes: [], edges: [] } as Graph },
    parameters: [],
    references: [],
    settings: settings(),
  };
}

function wholePass(id: string, source: string): Pass {
  return {
    id,
    name: id,
    source: { kind: "wholePassCode", source, filename: "x.slang", opaque: true },
    parameters: [],
    references: [],
    settings: settings(),
  };
}

function project(passes: Pass[]): Project {
  return { ...EMPTY_PROJECT, passes };
}

describe("substituteGraphPasses", () => {
  it("replaces a graph pass with whole-pass code carrying its generated source", () => {
    const proj = project([graphPass("g")]);
    const out = substituteGraphPasses(proj, { g: "#version 450\n// generated" });
    expect(out.ok).toBe(true);
    if (!out.ok) return;
    const pass = out.project.passes[0]!;
    expect(pass.source.kind).toBe("wholePassCode");
    if (pass.source.kind === "wholePassCode") {
      expect(pass.source.source).toBe("#version 450\n// generated");
      expect(pass.source.opaque).toBe(true);
    }
  });

  it("leaves an existing whole-pass code pass untouched", () => {
    const proj = project([wholePass("w", "VERBATIM")]);
    const out = substituteGraphPasses(proj, {});
    expect(out.ok).toBe(true);
    if (!out.ok) return;
    expect(out.project.passes[0]!.source).toEqual(proj.passes[0]!.source);
  });

  it("does NOT mutate the input project", () => {
    const proj = project([graphPass("g")]);
    substituteGraphPasses(proj, { g: "// gen" });
    expect(proj.passes[0]!.source.kind).toBe("graph");
  });

  it("reports uncompiled graph passes as a blocker", () => {
    const proj = project([graphPass("a"), graphPass("b")]);
    const out = substituteGraphPasses(proj, { a: "// ok", b: null });
    expect(out.ok).toBe(false);
    if (out.ok) return;
    expect(out.uncompiledPassIds).toEqual(["b"]);
  });

  it("treats a missing source entry as uncompiled", () => {
    const proj = project([graphPass("a")]);
    const out = substituteGraphPasses(proj, {});
    expect(out.ok).toBe(false);
  });
});

describe("sourcesFromCompile", () => {
  it("indexes generated sources by pass id", () => {
    expect(
      sourcesFromCompile([
        { passId: "a", source: "// a" },
        { passId: "b", source: null },
      ]),
    ).toEqual({ a: "// a", b: null });
  });
});
