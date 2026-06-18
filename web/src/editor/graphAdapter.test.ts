// graphAdapter edge-legality marking (#65): an edge made illegal by a later
// node-data change is tagged inline, and an implicitly-coerced edge is annotated.
import { describe, expect, it } from "vitest";

import type { Edge } from "../bindings/Edge";
import type { Graph } from "../bindings/Graph";
import type { Node } from "../bindings/Node";
import { toRfEdge, toRfGraph } from "./graphAdapter";

function node(id: string, kind: string, data: Record<string, unknown> = {}): Node {
  return { id, kind, position: { x: 0, y: 0 }, data };
}

function edge(id: string, source: string, sourcePort: string, target: string, targetPort: string): Edge {
  return { id, source, sourcePort, target, targetPort };
}

const emptySelection = { nodeIds: [], edgeIds: [] };

describe("toRfEdge — type-legality marking", () => {
  it("flags an edge made illegal by a later node-data change", () => {
    // Combine retargeted to vec3 now feeds an Output(vec4) color sink.
    const graph: Graph = {
      nodes: [node("c", "combine", { ty: "vec3" }), node("o", "output")],
      edges: [edge("e1", "c", "out", "o", "color")],
    };
    const rf = toRfEdge(graph.edges[0]!, false, graph);
    expect(rf.className).toContain("editor-edge--invalid");
    expect(rf.label).toBe("type mismatch");
  });

  it("annotates an int→float widen edge", () => {
    const graph: Graph = {
      nodes: [
        node("i", "const", { constType: "int", value: 1 }),
        node("m", "math", { op: "add" }),
      ],
      // Math operand 'a' is a polymorphic Expr operand — legal, but coercion is
      // only marked for fixed `assignable` sinks; an operand reads coercion none.
      edges: [edge("e1", "i", "out", "m", "a")],
    };
    const rf = toRfEdge(graph.edges[0]!, false, graph);
    // Expr operand → no coercion marking (polymorphic).
    expect(rf.className).not.toContain("editor-edge--widen");
    expect(rf.label).toBeUndefined();
  });

  it("annotates a float→vecN broadcast into a fixed sink", () => {
    const graph: Graph = {
      nodes: [node("f", "const", { constType: "float", value: 1 }), node("o", "output")],
      edges: [edge("e1", "f", "out", "o", "color")],
    };
    const rf = toRfEdge(graph.edges[0]!, false, graph);
    expect(rf.className).toContain("editor-edge--broadcast");
    expect(rf.label).toBe("→vec4");
  });

  it("leaves an exact-typed edge unmarked", () => {
    const graph: Graph = {
      nodes: [node("uv", "texcoord"), node("s", "source")],
      edges: [edge("e1", "uv", "out", "s", "coord")],
    };
    const rf = toRfEdge(graph.edges[0]!, false, graph);
    expect(rf.className).toBe("editor-edge");
    expect(rf.label).toBeUndefined();
  });

  it("does not flag an edge it cannot judge", () => {
    const graph: Graph = {
      nodes: [node("x", "unknownKind"), node("o", "output")],
      edges: [edge("e1", "x", "out", "o", "color")],
    };
    const rf = toRfEdge(graph.edges[0]!, false, graph);
    expect(rf.className).toBe("editor-edge");
  });
});

describe("toRfGraph — passes the live graph to edge marking", () => {
  it("tags a stale edge across the full graph projection", () => {
    const graph: Graph = {
      nodes: [node("c", "combine", { ty: "vec3" }), node("o", "output")],
      edges: [edge("e1", "c", "out", "o", "color")],
    };
    const { edges } = toRfGraph(graph, emptySelection);
    expect(edges[0]!.className).toContain("editor-edge--invalid");
  });
});
