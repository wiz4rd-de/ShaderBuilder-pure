import { beforeEach, describe, expect, it } from "vitest";

import type { Node } from "../bindings/Node";
import type { Pass } from "../bindings/Pass";
import type { Project } from "../bindings/Project";
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
