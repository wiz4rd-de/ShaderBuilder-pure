import { beforeEach, describe, expect, it } from "vitest";

import type { Edge } from "../bindings/Edge";
import type { Graph } from "../bindings/Graph";
import type { Node } from "../bindings/Node";
import type { Subgraph } from "../bindings/Subgraph";
import { defaultDataFor } from "../nodes/registry";
import { graphToIrGraph } from "../nodes/graphToIr";
import { isSubgraphNode } from "../nodes/subgraph";
import { collapseSelection, expandSubgraph } from "./collapse";
import { nextId, resetIdsForTest } from "./ids";

beforeEach(() => resetIdsForTest());

function node(id: string, kind: string, data: Record<string, unknown> = {}): Node {
  return { id, kind, position: { x: 0, y: 0 }, data: { ...defaultDataFor(kind), ...data } };
}
function edge(id: string, source: string, sourcePort: string, target: string, targetPort: string): Edge {
  return { id, source, sourcePort, target, targetPort };
}

/** Texcoord → Sample(Source) → Output. We collapse the Sample alone. */
function sampleChain(): Graph {
  return {
    nodes: [node("tc", "texcoord"), node("src", "source"), node("out", "output")],
    edges: [
      edge("e1", "tc", "uv", "src", "coord"),
      edge("e2", "src", "out", "out", "color"),
    ],
  };
}

describe("collapse — boundary derivation", () => {
  it("collapses a node, deriving correctly-typed in/out boundary ports", () => {
    const g = sampleChain();
    const res = collapseSelection(g, ["src"], "MySub", nextId)!;
    expect(res).not.toBeNull();

    // The Sample node is replaced by one subgraph node.
    const sgNode = res.graph.nodes.find((n) => n.id === res.subgraphNodeId)!;
    expect(isSubgraphNode(sgNode)).toBe(true);
    expect(res.graph.nodes.some((n) => n.id === "src")).toBe(false);

    const sub = sgNode.data as unknown as Subgraph;
    expect(sub.name).toBe("MySub");
    expect(sub.nodes.map((n) => n.id)).toEqual(["src"]);

    // One IN boundary (tc → src.coord, a vec2) and one OUT boundary (src.out → out.color, a vec4).
    const inPorts = sub.boundaryPorts.filter((b) => b.direction === "in");
    const outPorts = sub.boundaryPorts.filter((b) => b.direction === "out");
    expect(inPorts).toHaveLength(1);
    expect(outPorts).toHaveLength(1);
    expect(inPorts[0]!.ty).toBe("vec2");
    expect(inPorts[0]!.interiorNode).toBe("src");
    expect(inPorts[0]!.interiorPort).toBe("coord");
    expect(outPorts[0]!.ty).toBe("vec4");
    expect(outPorts[0]!.interiorPort).toBe("out");

    // The parent edges are rewired onto the new node's boundary ports.
    const intoSg = res.graph.edges.find((e) => e.target === res.subgraphNodeId)!;
    expect(intoSg.source).toBe("tc");
    expect(intoSg.targetPort).toBe(inPorts[0]!.name);
    const outOfSg = res.graph.edges.find((e) => e.source === res.subgraphNodeId)!;
    expect(outOfSg.target).toBe("out");
    expect(outOfSg.sourcePort).toBe(outPorts[0]!.name);
  });

  it("returns null on an empty selection", () => {
    expect(collapseSelection(sampleChain(), [], "X", nextId)).toBeNull();
  });

  it("moves edges fully inside the selection into the interior", () => {
    const g = sampleChain();
    // Collapse tc + src together: the tc→src edge is interior, src→out crosses.
    const res = collapseSelection(g, ["tc", "src"], "Pair", nextId)!;
    const sub = res.graph.nodes.find((n) => n.id === res.subgraphNodeId)!.data as unknown as Subgraph;
    expect(sub.edges.map((e) => e.id)).toEqual(["e1"]);
    // Only the crossing src→out becomes a boundary (an OUT port).
    expect(sub.boundaryPorts).toHaveLength(1);
    expect(sub.boundaryPorts[0]!.direction).toBe("out");
  });
});

describe("collapse → expand round-trip", () => {
  /** Structural signature: node kinds (sorted) + edges by endpoint kinds/ports. */
  function topology(g: Graph): { kinds: string[]; edges: string[] } {
    const kindById = new Map(g.nodes.map((n) => [n.id, n.kind]));
    return {
      kinds: g.nodes.map((n) => n.kind).sort(),
      edges: g.edges
        .map((e) => `${kindById.get(e.source)}#${e.sourcePort} -> ${kindById.get(e.target)}#${e.targetPort}`)
        .sort(),
    };
  }

  it("restores a structurally-equivalent graph (same kinds + edge topology modulo ids)", () => {
    const original = sampleChain();
    const collapsed = collapseSelection(original, ["src"], "S", nextId)!;
    expect(collapsed.graph.nodes.some(isSubgraphNode)).toBe(true);

    const expanded = expandSubgraph(collapsed.graph, collapsed.subgraphNodeId, nextId)!;
    expect(expanded).not.toBeNull();
    expect(expanded.nodes.some(isSubgraphNode)).toBe(false);
    expect(topology(expanded)).toEqual(topology(original));
  });

  it("expand returns null for a non-subgraph node", () => {
    expect(expandSubgraph(sampleChain(), "src", nextId)).toBeNull();
  });
});

describe("collapse — IR equivalence (compiles identically)", () => {
  /** Canonical id-independent IR signature (ops + op-anchored edges). */
  function irSig(g: Graph): { ops: string[]; edges: string[] } {
    const ir = graphToIrGraph(g);
    const opById = new Map(ir.nodes.map((n) => [n.id, JSON.stringify(n.op)]));
    return {
      ops: ir.nodes.map((n) => JSON.stringify(n.op)).sort(),
      edges: ir.edges
        .map((e) => `${opById.get(e.source.node)}#${e.source.port} -> ${opById.get(e.target.node)}#${e.target.port}`)
        .sort(),
    };
  }

  it("a collapsed graph lowers to the same IR as the original", () => {
    const original = sampleChain();
    const collapsed = collapseSelection(original, ["src"], "S", nextId)!;
    expect(irSig(collapsed.graph)).toEqual(irSig(original));
  });
});
