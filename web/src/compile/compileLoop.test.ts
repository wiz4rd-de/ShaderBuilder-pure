import { describe, expect, it, vi } from "vitest";

import type { CompileGraphResult } from "../bindings/CompileGraphResult";
import type { Diagnostic } from "../bindings/Diagnostic";
import type { Graph } from "../bindings/Graph";
import type { Pass } from "../bindings/Pass";
import type { PassSettings } from "../bindings/PassSettings";
import type { Project } from "../bindings/Project";
import { EMPTY_PROJECT } from "../model";
import { compileProject, planProject, type InvokeCompile } from "./compileLoop";

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

function graphPass(id: string, graph: Graph, overrides: Partial<Pass> = {}): Pass {
  return {
    id,
    name: id,
    source: { kind: "graph", graph },
    parameters: [],
    references: [],
    settings: settings(),
    ...overrides,
  };
}

function wholePass(id: string, source: string): Pass {
  return {
    id,
    name: id,
    source: { kind: "wholePassCode", source, filename: null, opaque: true },
    parameters: [],
    references: [],
    settings: settings(),
  };
}

function project(passes: Pass[]): Project {
  return { ...EMPTY_PROJECT, passes };
}

/** A Texcoord→Sample(Source)→Output graph (the canonical clean single-pass graph). */
function sampleGraph(): Graph {
  return {
    nodes: [
      { id: "coord", kind: "texcoord", position: { x: 0, y: 0 }, data: {} },
      { id: "src", kind: "source", position: { x: 0, y: 0 }, data: {} },
      { id: "out", kind: "output", position: { x: 0, y: 0 }, data: {} },
    ],
    edges: [
      { id: "e1", source: "coord", sourcePort: "uv", target: "src", targetPort: "coord" },
      { id: "e2", source: "src", sourcePort: "out", target: "out", targetPort: "color" },
    ],
  };
}

/** A fake compile_graph that returns a clean source for every call. */
function cleanCompile(source = "// slang"): InvokeCompile {
  return vi.fn(async () => ({ source, diagnostics: { items: [] } }) as CompileGraphResult);
}

/** A fake compile_graph that returns the given diagnostics with no source. */
function erroredCompile(items: Diagnostic[]): InvokeCompile {
  return vi.fn(async () => ({ source: null, diagnostics: { items } }) as CompileGraphResult);
}

describe("planProject", () => {
  it("plans a graph pass to an IrGraph and a whole-pass to its verbatim source", () => {
    const proj = project([graphPass("g", sampleGraph()), wholePass("w", "#version 450\n")]);
    const [g, w] = planProject(proj);
    expect(g!.graph).not.toBeNull();
    expect(g!.graph!.ir.nodes.length).toBe(3);
    expect(g!.wholePassSource).toBeNull();
    expect(w!.graph).toBeNull();
    expect(w!.wholePassSource).toBe("#version 450\n");
  });

  it("carries the pass alias as the #pragma name", () => {
    const pass = graphPass("g", sampleGraph(), {
      settings: { ...settings(), alias: "crtPass" },
    });
    const [plan] = planProject(project([pass]));
    expect(plan!.name).toBe("crtPass");
  });
});

describe("compileProject", () => {
  it("compiles a clean single-pass graph to a valid renderable result", async () => {
    const compile = cleanCompile("// generated");
    const result = await compileProject(project([graphPass("g", sampleGraph())]), compile);
    expect(compile).toHaveBeenCalledTimes(1);
    expect(result.valid).toBe(true);
    expect(result.passes[0]!.source).toBe("// generated");
    expect(result.problems).toHaveLength(0);
    expect(result.diagnosticsByNode).toEqual({});
  });

  it("uses a whole-pass pass's verbatim source without calling compile_graph", async () => {
    const compile = cleanCompile();
    const result = await compileProject(project([wholePass("w", "VERBATIM")]), compile);
    expect(compile).not.toHaveBeenCalled();
    expect(result.passes[0]!.source).toBe("VERBATIM");
    expect(result.valid).toBe(true);
  });

  it("maps diagnostics to the offending node id and flags the pipeline invalid", async () => {
    const diag: Diagnostic = {
      severity: "error",
      code: "typeMismatch",
      message: "expected vec4",
      node: "src",
      port: "coord",
    };
    const compile = erroredCompile([diag]);
    const result = await compileProject(project([graphPass("g", sampleGraph())]), compile);
    expect(result.valid).toBe(false);
    expect(result.diagnosticsByNode["src"]).toEqual([diag]);
    expect(result.problems).toEqual([
      { passId: "g", passName: "g", diagnostic: diag },
    ]);
  });

  it("compiles EACH graph pass and aggregates diagnostics per pass", async () => {
    const proj = project([
      graphPass("a", sampleGraph()),
      graphPass("b", sampleGraph()),
    ]);
    const compile = cleanCompile();
    await compileProject(proj, compile);
    expect(compile).toHaveBeenCalledTimes(2);
  });

  it("a single errored pass in a multi-pass pipeline makes the whole pipeline invalid", async () => {
    const proj = project([graphPass("a", sampleGraph()), graphPass("b", sampleGraph())]);
    let call = 0;
    const compile: InvokeCompile = vi.fn(async () => {
      call += 1;
      if (call === 2) {
        return {
          source: null,
          diagnostics: {
            items: [
              { severity: "error", code: "cycle", message: "cycle", node: "src", port: null },
            ],
          },
        } as CompileGraphResult;
      }
      return { source: "// ok", diagnostics: { items: [] } } as CompileGraphResult;
    });
    const result = await compileProject(proj, compile);
    expect(result.valid).toBe(false);
    expect(result.passes[0]!.source).toBe("// ok");
    expect(result.passes[1]!.source).toBeNull();
  });

  it("surfaces a graphToIr lowering issue as a node-keyed diagnostic", async () => {
    // An unregistered kind is dropped by graphToIr with an issue → synthetic diag.
    const graph: Graph = {
      nodes: [{ id: "bad", kind: "noSuchKind", position: { x: 0, y: 0 }, data: {} }],
      edges: [],
    };
    const compile = cleanCompile();
    const result = await compileProject(project([graphPass("g", graph)]), compile);
    expect(result.diagnosticsByNode["bad"]?.[0]?.code).toBe("unknownKind");
    expect(result.problems.some((p) => p.diagnostic.node === "bad")).toBe(true);
  });

  it("refuses to dispatch a pass whose sampler references a removed pass (#2/#3)", async () => {
    // removePass writes DANGLING_INDEX (-1) into a sampler whose pass was deleted
    // (pipeline/passOps.ts). The bridge/compile layer must SURFACE this as a
    // node-level error and NOT dispatch a mis-wired chain to the preview — even
    // though the (fake here, real in prod) compile_graph still returns a source.
    const graph: Graph = {
      nodes: [
        { id: "coord", kind: "texcoord", position: { x: 0, y: 0 }, data: {} },
        // index -1 = DANGLING_INDEX: would clamp to PassOutput0 under the old bug.
        { id: "samp", kind: "passOutput", position: { x: 0, y: 0 }, data: { index: -1 } },
        { id: "out", kind: "output", position: { x: 0, y: 0 }, data: {} },
      ],
      edges: [
        { id: "e1", source: "coord", sourcePort: "uv", target: "samp", targetPort: "coord" },
        { id: "e2", source: "samp", sourcePort: "out", target: "out", targetPort: "color" },
      ],
    };
    // cleanCompile would return a source for the (incomplete) graph; the refusal
    // must come from the FRONTEND, not from compile_graph.
    const compile = cleanCompile("// would-be-mis-wired");
    const result = await compileProject(project([graphPass("g", graph)]), compile);
    // The offending sampler node carries an error diagnostic.
    const diag = result.diagnosticsByNode["samp"]?.[0];
    expect(diag?.severity).toBe("error");
    expect(diag?.message).toMatch(/removed pass/);
    expect(result.problems.some((p) => p.diagnostic.node === "samp")).toBe(true);
    // The mis-wired chain is NOT renderable / NOT dispatched to the preview.
    expect(result.passes[0]!.source).toBeNull();
    expect(result.valid).toBe(false);
  });
});
