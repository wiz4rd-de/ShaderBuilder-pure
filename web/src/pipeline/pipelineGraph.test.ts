import { beforeEach, describe, expect, it } from "vitest";

import type { Node } from "../bindings/Node";
import type { Pass } from "../bindings/Pass";
import type { Project } from "../bindings/Project";
import { emptyPassSettings, makeProject } from "../store/factories";
import { resetIdsForTest } from "../store/ids";
import { toPipelineGraph } from "./pipelineGraph";

function node(id: string, kind: string, data: Record<string, unknown> = {}): Node {
  return { id, kind, position: { x: 0, y: 0 }, data };
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

function project(passes: Pass[]): Project {
  return { ...makeProject(), passes };
}

beforeEach(() => resetIdsForTest());

describe("toPipelineGraph", () => {
  it("derives one node per pass in pass-index order", () => {
    const p = project([graphPass("a", "A", []), graphPass("b", "B", [])]);
    const { nodes } = toPipelineGraph(p, null);
    expect(nodes.map((n) => n.id)).toEqual(["a", "b"]);
    expect(nodes[0]!.data.index).toBe(0);
    expect(nodes[1]!.data.index).toBe(1);
  });

  it("emits a passOutput edge from producer to consumer", () => {
    const p = project([
      graphPass("a", "A", []),
      graphPass("b", "B", [node("s", "passOutput", { index: 0 })]),
    ]);
    const { edges } = toPipelineGraph(p, null);
    expect(edges).toHaveLength(1);
    expect(edges[0]!.source).toBe("a");
    expect(edges[0]!.target).toBe("b");
    expect(edges[0]!.targetHandle).toBe("passOutput");
    expect(edges[0]!.data?.binding).toBe("passOutput");
  });

  it("distinguishes feedback from pass-output edges (style + animated)", () => {
    const p = project([
      graphPass("a", "A", []),
      graphPass("b", "B", [node("s", "passFeedback", { index: 0 })]),
    ]);
    const { edges } = toPipelineGraph(p, null);
    expect(edges).toHaveLength(1);
    expect(edges[0]!.data?.binding).toBe("passFeedback");
    expect(edges[0]!.className).toContain("pipeline-edge--passFeedback");
    expect(edges[0]!.animated).toBe(true);
  });

  it("drops edges for out-of-range / dangling index refs", () => {
    const p = project([
      graphPass("a", "A", []),
      graphPass("b", "B", [node("s", "passOutput", { index: 5 })]),
    ]);
    const { edges } = toPipelineGraph(p, null);
    expect(edges).toHaveLength(0);
  });

  it("records boundary inputs (Source/Original/History/LUT) as node chips", () => {
    const p = project([
      graphPass("a", "A", [
        node("src", "source"),
        node("hist", "originalHistory", { index: 2 }),
        node("lut", "lut", { name: "BORDER" }),
      ]),
    ]);
    const { nodes } = toPipelineGraph(p, null);
    const inputs = nodes[0]!.data.boundaryInputs;
    expect(inputs).toContainEqual({ kind: "source" });
    expect(inputs).toContainEqual({ kind: "originalHistory", detail: "2" });
    expect(inputs).toContainEqual({ kind: "lut", detail: "BORDER" });
  });

  it("marks the selected pass node", () => {
    const p = project([graphPass("a", "A", []), graphPass("b", "B", [])]);
    const { nodes } = toPipelineGraph(p, "b");
    expect(nodes.find((n) => n.id === "a")!.selected).toBe(false);
    expect(nodes.find((n) => n.id === "b")!.selected).toBe(true);
  });
});
