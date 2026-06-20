import { describe, expect, it } from "vitest";

import type { BoundaryPort } from "../bindings/BoundaryPort";
import type { Edge } from "../bindings/Edge";
import type { Graph } from "../bindings/Graph";
import type { Node } from "../bindings/Node";
import type { Subgraph } from "../bindings/Subgraph";
import { defaultDataFor } from "./registry";
import {
  inlineAllSubgraphs,
  isSubgraphNode,
  readSubgraph,
  SUBGRAPH_KIND,
  type MintId,
} from "./subgraph";

/** A fresh deterministic minter (mirrors graphToIr's stable id scheme). */
function counter(): MintId {
  let n = 0;
  return (prefix) => {
    n += 1;
    return `inl-${prefix}-${n}`;
  };
}

function node(id: string, kind: string, data: Record<string, unknown> = {}): Node {
  return { id, kind, position: { x: 0, y: 0 }, data: { ...defaultDataFor(kind), ...data } };
}

function edge(id: string, source: string, sourcePort: string, target: string, targetPort: string): Edge {
  return { id, source, sourcePort, target, targetPort };
}

function subgraphNode(id: string, sub: Subgraph): Node {
  return { id, kind: SUBGRAPH_KIND, position: { x: 0, y: 0 }, data: sub as unknown as Record<string, unknown> };
}

describe("subgraph — readSubgraph / isSubgraphNode", () => {
  it("recognizes a collapsed subgraph node", () => {
    expect(isSubgraphNode(subgraphNode("s1", emptySub()))).toBe(true);
    expect(isSubgraphNode(node("n1", "source"))).toBe(false);
  });

  it("coerces a partial/empty data to an empty subgraph", () => {
    const sub = readSubgraph({ id: "x", kind: "subgraph", position: { x: 0, y: 0 }, data: {} });
    expect(sub.nodes).toEqual([]);
    expect(sub.edges).toEqual([]);
    expect(sub.boundaryPorts).toEqual([]);
  });
});

function emptySub(): Subgraph {
  return { id: "s", name: "S", nodes: [], edges: [], boundaryPorts: [] };
}

/**
 * Build a subgraph wrapping a single `customSnippet` interior node that maps a
 * vec2 `coord` input boundary to a vec4 `result` output boundary, plus the
 * collapsed graph (an external Source feeds the boundary `coord`; the boundary
 * `result` feeds an Output).
 */
function makeCollapsedFixture(): {
  collapsed: Graph;
  interiorId: string;
} {
  const interiorId = "int-snip";
  const boundaryPorts: BoundaryPort[] = [
    { name: "coordIn", ty: "vec2", direction: "in", interiorNode: interiorId, interiorPort: "coord" },
    { name: "colorOut", ty: "vec4", direction: "out", interiorNode: interiorId, interiorPort: "result" },
  ];
  const sub: Subgraph = {
    id: "sub-1",
    name: "MySub",
    nodes: [
      node(interiorId, "customSnippet", {
        body: "result = texture(Source, coord);",
        inputs: [{ name: "coord", type: "vec2" }],
        outputs: [{ name: "result", type: "vec4" }],
      }),
    ],
    edges: [],
    boundaryPorts,
  };
  const collapsed: Graph = {
    nodes: [
      node("tc", "texcoord"),
      subgraphNode("sg", sub),
      node("out", "output"),
    ],
    edges: [
      edge("e1", "tc", "uv", "sg", "coordIn"),
      edge("e2", "sg", "colorOut", "out", "color"),
    ],
  };
  return { collapsed, interiorId };
}

describe("subgraph — inlineAllSubgraphs", () => {
  it("replaces a subgraph node with its interior and rewires boundary edges", () => {
    const { collapsed } = makeCollapsedFixture();
    const inlined = inlineAllSubgraphs(collapsed, counter());

    // No subgraph nodes remain; the interior snippet is spliced in.
    expect(inlined.nodes.some(isSubgraphNode)).toBe(false);
    const kinds = inlined.nodes.map((n) => n.kind).sort();
    expect(kinds).toEqual(["customSnippet", "output", "texcoord"]);

    // The interior node got a fresh id (not the original "int-snip").
    const snip = inlined.nodes.find((n) => n.kind === "customSnippet")!;
    expect(snip.id).not.toBe("int-snip");

    // The "coord" feed now targets the interior snippet's coord port directly.
    const intoSnip = inlined.edges.find((e) => e.target === snip.id);
    expect(intoSnip).toMatchObject({ source: "tc", sourcePort: "uv", targetPort: "coord" });

    // The "result" output now sources straight from the interior snippet.
    const outOfSnip = inlined.edges.find((e) => e.source === snip.id);
    expect(outOfSnip).toMatchObject({ sourcePort: "result", target: "out", targetPort: "color" });
  });

  it("is a no-op (same topology) on a graph with no subgraph nodes", () => {
    const g: Graph = {
      nodes: [node("a", "source"), node("b", "output")],
      edges: [edge("e", "a", "out", "b", "color")],
    };
    const inlined = inlineAllSubgraphs(g, counter());
    expect(inlined.nodes.map((n) => n.id)).toEqual(["a", "b"]);
    expect(inlined.edges).toEqual(g.edges);
  });

  it("recursively inlines a nested subgraph", () => {
    // Outer subgraph whose only interior node is ITSELF a collapsed subgraph.
    const innerInteriorId = "inner-snip";
    const inner: Subgraph = {
      id: "inner",
      name: "Inner",
      nodes: [
        node(innerInteriorId, "customSnippet", {
          body: "result = vec4(coord, 0.0, 1.0);",
          inputs: [{ name: "coord", type: "vec2" }],
          outputs: [{ name: "result", type: "vec4" }],
        }),
      ],
      edges: [],
      boundaryPorts: [
        { name: "cIn", ty: "vec2", direction: "in", interiorNode: innerInteriorId, interiorPort: "coord" },
        { name: "rOut", ty: "vec4", direction: "out", interiorNode: innerInteriorId, interiorPort: "result" },
      ],
    };
    const innerNodeId = "inner-node";
    const outer: Subgraph = {
      id: "outer",
      name: "Outer",
      nodes: [subgraphNode(innerNodeId, inner)],
      edges: [],
      boundaryPorts: [
        { name: "oIn", ty: "vec2", direction: "in", interiorNode: innerNodeId, interiorPort: "cIn" },
        { name: "oOut", ty: "vec4", direction: "out", interiorNode: innerNodeId, interiorPort: "rOut" },
      ],
    };
    const g: Graph = {
      nodes: [node("tc", "texcoord"), subgraphNode("outerNode", outer), node("out", "output")],
      edges: [
        edge("e1", "tc", "uv", "outerNode", "oIn"),
        edge("e2", "outerNode", "oOut", "out", "color"),
      ],
    };
    const inlined = inlineAllSubgraphs(g, counter());
    expect(inlined.nodes.some(isSubgraphNode)).toBe(false);
    const snip = inlined.nodes.find((n) => n.kind === "customSnippet")!;
    expect(snip).toBeDefined();
    // Boundary chain coord → cIn → oIn collapses to a direct tc.uv → snip.coord edge.
    const intoSnip = inlined.edges.find((e) => e.target === snip.id);
    expect(intoSnip).toMatchObject({ source: "tc", sourcePort: "uv", targetPort: "coord" });
    const outOfSnip = inlined.edges.find((e) => e.source === snip.id);
    expect(outOfSnip).toMatchObject({ sourcePort: "result", target: "out", targetPort: "color" });
  });
});
