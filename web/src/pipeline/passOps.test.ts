import { beforeEach, describe, expect, it, vi } from "vitest";

import type { CompileGraphResult } from "../bindings/CompileGraphResult";
import type { Node } from "../bindings/Node";
import type { Pass } from "../bindings/Pass";
import type { Project } from "../bindings/Project";
import { compileProject, type InvokeCompile } from "../compile/compileLoop";
import { getDescriptor } from "../nodes/registry";
import { emptyPassSettings, makeProject } from "../store/factories";
import { resetIdsForTest } from "../store/ids";
import {
  addPass,
  DANGLING_INDEX,
  passToGraph,
  passToWholePassCode,
  removePass,
  reorderPass,
  setWholePassSource,
} from "./passOps";

// A sampler node binding an earlier pass BY INDEX (PassOutputN / PassFeedbackN).
function indexSampler(id: string, kind: "passOutput" | "passFeedback", index: number): Node {
  return { id, kind, position: { x: 0, y: 0 }, data: { index } };
}

function graphPass(id: string, name: string, nodes: Node[]): Pass {
  return {
    id,
    name,
    source: { kind: "graph", graph: { nodes, edges: [] } },
    parameters: [],
    settings: emptyPassSettings(),
    references: [],
  };
}

/** Read the stored index off a pass's sampler node. */
function sampledIndex(pass: Pass, nodeId: string): number {
  const node =
    pass.source.kind === "graph"
      ? pass.source.graph.nodes.find((n) => n.id === nodeId)
      : undefined;
  return (node?.data as { index: number }).index;
}

beforeEach(() => resetIdsForTest());

describe("passOps — addPass", () => {
  it("appends without disturbing existing pass indices/refs", () => {
    const base = makeProject();
    const next = addPass(base, graphPass("pass-x", "Pass 2", []));
    expect(next.passes).toHaveLength(2);
    expect(next.passes[1]!.id).toBe("pass-x");
    // base untouched (pure).
    expect(base.passes).toHaveLength(1);
  });
});

describe("passOps — reorderPass remaps index refs", () => {
  // 3 passes: pass0, pass1 (samples PassOutput0), pass2 (samples PassFeedback1).
  function chain(): Project {
    const base = makeProject();
    return {
      ...base,
      feedbackPass: 1,
      passes: [
        graphPass("p0", "P0", []),
        graphPass("p1", "P1", [indexSampler("s1", "passOutput", 0)]),
        graphPass("p2", "P2", [indexSampler("s2", "passFeedback", 1)]),
      ],
    };
  }

  it("moving a referenced pass remaps the consuming sampler's index", () => {
    const project = chain();
    // Move p0 (index 0) to the end (index 2). Order becomes p1, p2, p0.
    const next = reorderPass(project, 0, 2);
    expect(next.passes.map((p) => p.id)).toEqual(["p1", "p2", "p0"]);
    // p1 sampled PassOutput of p0; p0 is now index 2.
    const p1 = next.passes.find((p) => p.id === "p1")!;
    expect(sampledIndex(p1, "s1")).toBe(2);
    // p2 sampled PassFeedback of p1; p1 is now index 0.
    const p2 = next.passes.find((p) => p.id === "p2")!;
    expect(sampledIndex(p2, "s2")).toBe(0);
  });

  it("remaps the global feedbackPass index too", () => {
    const project = chain();
    const next = reorderPass(project, 1, 0); // p1 → front; order p1,p0,p2
    // feedbackPass was 1 (p1); p1 is now index 0.
    expect(next.feedbackPass).toBe(0);
  });

  it("a no-op / out-of-range move returns the project unchanged", () => {
    const project = chain();
    expect(reorderPass(project, 1, 1)).toBe(project);
    expect(reorderPass(project, -1, 0)).toBe(project);
    expect(reorderPass(project, 0, 9)).toBe(project);
  });
});

describe("passOps — removePass remaps + dangles", () => {
  function chain(): Project {
    const base = makeProject();
    return {
      ...base,
      passes: [
        graphPass("p0", "P0", []),
        graphPass("p1", "P1", [indexSampler("s1", "passOutput", 0)]),
        graphPass("p2", "P2", [indexSampler("s2", "passOutput", 1)]),
      ],
    };
  }

  it("shifts indices down and remaps surviving refs", () => {
    const project = chain();
    // Remove p0; order becomes p1, p2. p2 referenced p1 (was index 1, now 0).
    const next = removePass(project, "p0");
    expect(next.passes.map((p) => p.id)).toEqual(["p1", "p2"]);
    const p2 = next.passes.find((p) => p.id === "p2")!;
    expect(sampledIndex(p2, "s2")).toBe(0);
  });

  it("a ref to the removed pass becomes the dangling sentinel", () => {
    const project = chain();
    // Remove p0, which p1 referenced (PassOutput0).
    const next = removePass(project, "p0");
    const p1 = next.passes.find((p) => p.id === "p1")!;
    expect(sampledIndex(p1, "s1")).toBe(DANGLING_INDEX);
  });

  it("the dangling sentinel SURFACES as a compile refusal, not a silent index 0 (#3)", async () => {
    // The data-layer test above only proves removePass STORES the sentinel; this
    // proves the policy promised in passOps.ts — the downstream compile surfaces
    // it as an error rather than silently re-pointing the chain at PassOutput0.
    //
    // A 2-pass chain: p0, and p1 whose graph samples PassOutput0 (= p0).
    const base = makeProject();
    const samplerGraph = (index: number): Pass => ({
      id: "p1",
      name: "P1",
      source: {
        kind: "graph",
        graph: {
          nodes: [
            { id: "coord", kind: "texcoord", position: { x: 0, y: 0 }, data: {} },
            { id: "s1", kind: "passOutput", position: { x: 0, y: 0 }, data: { index } },
            { id: "out", kind: "output", position: { x: 0, y: 0 }, data: {} },
          ],
          edges: [
            { id: "e1", source: "coord", sourcePort: "uv", target: "s1", targetPort: "coord" },
            { id: "e2", source: "s1", sourcePort: "out", target: "out", targetPort: "color" },
          ],
        },
      },
      parameters: [],
      settings: emptyPassSettings(),
      references: [],
    });
    const project: Project = {
      ...base,
      passes: [graphPass("p0", "P0", []), samplerGraph(0)],
    };
    // Removing p0 dangles p1's sampler (PassOutput0 → DANGLING_INDEX).
    const dangled = removePass(project, "p0");
    const p1 = dangled.passes.find((p) => p.id === "p1")!;
    expect(sampledIndex(p1, "s1")).toBe(DANGLING_INDEX);

    // toNodeOp must NOT silently yield texture index 0 for the dangling sampler —
    // it must throw so the bridge can surface a node diagnostic instead.
    expect(() =>
      getDescriptor("passOutput")!.toNodeOp({ index: DANGLING_INDEX }),
    ).toThrow(/removed pass/);

    // A compile_graph that would happily return a (mis-wired) source: the refusal
    // must come from the FRONTEND, not from compile_graph.
    const compile: InvokeCompile = vi.fn(
      async () => ({ source: "// would-be-mis-wired", diagnostics: { items: [] } }) as CompileGraphResult,
    );
    const result = await compileProject(dangled, compile);
    // The offending sampler node carries an inline error diagnostic.
    expect(result.diagnosticsByNode["s1"]?.[0]?.severity).toBe("error");
    expect(result.diagnosticsByNode["s1"]?.[0]?.message).toMatch(/removed pass/);
    // The mis-wired chain is invalid and never dispatched to the preview.
    expect(result.valid).toBe(false);
    const p1Result = result.passes.find((p) => p.passId === "p1")!;
    expect(p1Result.source).toBeNull();
  });

  it("refuses to remove the last remaining pass", () => {
    const base = makeProject();
    expect(removePass(base, base.passes[0]!.id)).toBe(base);
  });
});

describe("passOps — pass-source kind switching (#52)", () => {
  const SRC = "#version 450\n#pragma stage fragment\nvoid main() {}\n";

  it("passToWholePassCode replaces a graph pass with opaque code", () => {
    const pass = graphPass("p", "Pass 1", [indexSampler("s", "passOutput", 0)]);
    const next = passToWholePassCode(pass, SRC);
    expect(next.source.kind).toBe("wholePassCode");
    if (next.source.kind === "wholePassCode") {
      expect(next.source.source).toBe(SRC);
      expect(next.source.opaque).toBe(true);
      expect(next.source.filename).toBeNull();
    }
  });

  it("passToWholePassCode is a no-op when source already matches", () => {
    const pass = passToWholePassCode(graphPass("p", "Pass 1", []), SRC);
    expect(passToWholePassCode(pass, SRC)).toBe(pass);
  });

  it("passToWholePassCode preserves an existing filename", () => {
    const imported: Pass = {
      id: "p",
      name: "Imported",
      source: { kind: "wholePassCode", source: "old", filename: "crt.slang", opaque: true },
      parameters: [],
      settings: emptyPassSettings(),
      references: [],
    };
    const next = passToWholePassCode(imported, SRC);
    if (next.source.kind === "wholePassCode") {
      expect(next.source.filename).toBe("crt.slang");
    }
  });

  it("passToGraph converts a whole-pass code pass back to an empty graph", () => {
    const code = passToWholePassCode(graphPass("p", "Pass 1", []), SRC);
    const next = passToGraph(code);
    expect(next.source.kind).toBe("graph");
    if (next.source.kind === "graph") {
      expect(next.source.graph).toEqual({ nodes: [], edges: [] });
    }
  });

  it("passToGraph is a no-op for a pass that already is a graph", () => {
    const pass = graphPass("p", "Pass 1", []);
    expect(passToGraph(pass)).toBe(pass);
  });

  it("setWholePassSource edits the verbatim source, no-op when unchanged or a graph", () => {
    const code = passToWholePassCode(graphPass("p", "Pass 1", []), SRC);
    const edited = setWholePassSource(code, "changed");
    if (edited.source.kind === "wholePassCode") {
      expect(edited.source.source).toBe("changed");
    }
    expect(setWholePassSource(code, SRC)).toBe(code);
    const graph = graphPass("g", "Pass 2", []);
    expect(setWholePassSource(graph, "x")).toBe(graph);
  });
});
