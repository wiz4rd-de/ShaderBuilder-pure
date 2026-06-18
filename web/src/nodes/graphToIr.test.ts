import { describe, expect, it } from "vitest";

import type { BoundaryPort } from "../bindings/BoundaryPort";
import type { Edge } from "../bindings/Edge";
import type { Graph } from "../bindings/Graph";
import type { IrGraph } from "../bindings/IrGraph";
import type { Node } from "../bindings/Node";
import type { Subgraph } from "../bindings/Subgraph";
import { defaultDataFor } from "./registry";
import { SUBGRAPH_KIND } from "./subgraph";
import { graphToIr, graphToIrGraph } from "./graphToIr";

let seq = 0;
function node(kind: string, data: Record<string, unknown> = {}): Node {
  seq += 1;
  return {
    id: `${kind}-${seq}`,
    kind,
    position: { x: 0, y: 0 },
    data: { ...defaultDataFor(kind), ...data },
  };
}

function edge(source: string, sourcePort: string, target: string, targetPort: string): Edge {
  seq += 1;
  return { id: `edge-${seq}`, source, sourcePort, target, targetPort };
}

describe("graphToIr — node mapping", () => {
  it("maps each skeletal node to a typed IrNode by descriptor", () => {
    const tc = node("texcoord");
    const src = node("source");
    const out = node("output");
    const graph: Graph = {
      nodes: [tc, src, out],
      edges: [],
    };
    const { ir } = graphToIr(graph);
    expect(ir.nodes.map((n) => n.id)).toEqual([tc.id, src.id, out.id]);
    expect(ir.nodes.find((n) => n.id === src.id)!.op).toEqual({
      kind: "sample",
      texture: { kind: "source" },
    });
    expect(ir.nodes.find((n) => n.id === out.id)!.op).toEqual({ kind: "output" });
  });

  it("maps edges to PortRef→PortRef using source/target ports", () => {
    const tc = node("texcoord");
    const src = node("source");
    const graph: Graph = {
      nodes: [tc, src],
      edges: [edge(tc.id, "uv", src.id, "coord")],
    };
    const { ir } = graphToIr(graph);
    expect(ir.edges).toEqual([
      { source: { node: tc.id, port: "uv" }, target: { node: src.id, port: "coord" } },
    ]);
  });
});

describe("graphToIr — minimal round-trip graph (Texcoord → Sample(Source) → Output)", () => {
  function minimalGraph(): { graph: Graph; ids: Record<string, string> } {
    const tc = node("texcoord");
    const src = node("source");
    const out = node("output");
    const graph: Graph = {
      nodes: [tc, src, out],
      edges: [
        edge(tc.id, "uv", src.id, "coord"),
        edge(src.id, "out", out.id, "color"),
      ],
    };
    return { graph, ids: { tc: tc.id, src: src.id, out: out.id } };
  }

  it("produces the expected IrGraph shape the Phase-4 emit fixtures use", () => {
    const { graph, ids } = minimalGraph();
    const ir = graphToIrGraph(graph);

    // Three typed nodes: a coord source, a Source sample, and the Output sink.
    expect(ir.nodes).toHaveLength(3);

    const tc = ir.nodes.find((n) => n.id === ids.tc)!;
    expect(tc.op.kind).toBe("customSnippet");
    if (tc.op.kind === "customSnippet") {
      expect(tc.op.body).toContain("vTexCoord");
      expect(tc.op.outputs).toEqual([{ name: "uv", type: "vec2" }]);
    }

    expect(ir.nodes.find((n) => n.id === ids.src)!.op).toEqual({
      kind: "sample",
      texture: { kind: "source" },
    });
    expect(ir.nodes.find((n) => n.id === ids.out)!.op).toEqual({ kind: "output" });

    // The coord feeds Sample.coord and the sample feeds Output.color — exactly the
    // port names the #40 checker / #41 lowering agree on.
    expect(ir.edges).toEqual([
      { source: { node: ids.tc, port: "uv" }, target: { node: ids.src, port: "coord" } },
      { source: { node: ids.src, port: "out" }, target: { node: ids.out, port: "color" } },
    ]);
  });

  it("declares no parameters/LUTs and reports no issues for the clean graph", () => {
    const { graph } = minimalGraph();
    const result = graphToIr(graph);
    expect(result.parameters).toEqual([]);
    expect(result.luts).toEqual([]);
    expect(result.issues).toEqual([]);
  });
});

describe("graphToIr — parameters + LUTs collection", () => {
  it("collects a pass Parameter from a Param node (de-duped by name)", () => {
    const p1 = node("param", { name: "GAMMA", label: "Gamma", default: 1, min: 0.1, max: 3, step: 0.05 });
    const p2 = node("param", { name: "GAMMA", label: "Gamma again", default: 2, min: 0, max: 4, step: 0.1 });
    const graph: Graph = { nodes: [p1, p2], edges: [] };
    const { parameters } = graphToIr(graph);
    expect(parameters).toEqual([
      { name: "GAMMA", label: "Gamma", default: 1, min: 0.1, max: 3, step: 0.05 },
    ]);
  });

  it("ignores an unnamed Param node for the parameters list", () => {
    // An unnamed param still lowers its NodeOp throwing? No — toNodeOp requires a
    // name, so an unnamed param is DROPPED with an issue; it contributes nothing.
    const p = node("param", { name: "" });
    const graph: Graph = { nodes: [p], edges: [] };
    const { parameters, issues } = graphToIr(graph);
    expect(parameters).toEqual([]);
    expect(issues).toHaveLength(1);
    expect(issues[0]!.reason).toBe("loweringError");
  });

  it("collects referenced LUT names (de-duped, declared order)", () => {
    const tc = node("texcoord");
    const a = node("lut", { name: "overlay" });
    const b = node("lut", { name: "scanlines" });
    const c = node("lut", { name: "overlay" });
    const graph: Graph = {
      nodes: [tc, a, b, c],
      edges: [
        edge(tc.id, "uv", a.id, "coord"),
        edge(tc.id, "uv", b.id, "coord"),
        edge(tc.id, "uv", c.id, "coord"),
      ],
    };
    const { luts } = graphToIr(graph);
    expect(luts).toEqual(["overlay", "scanlines"]);
  });
});

describe("graphToIr — robustness", () => {
  it("drops an unknown-kind node and records an issue", () => {
    const good = node("source");
    const bad: Node = { id: "x", kind: "totally-unknown", position: { x: 0, y: 0 }, data: {} };
    const graph: Graph = { nodes: [good, bad], edges: [] };
    const { ir, issues } = graphToIr(graph);
    expect(ir.nodes.map((n) => n.id)).toEqual([good.id]);
    expect(issues).toEqual([
      expect.objectContaining({ nodeId: "x", kind: "totally-unknown", reason: "unknownKind" }),
    ]);
  });

  it("drops edges incident to a dropped node", () => {
    const good = node("output");
    const bad: Node = { id: "x", kind: "nope", position: { x: 0, y: 0 }, data: {} };
    const graph: Graph = {
      nodes: [good, bad],
      edges: [edge("x", "out", good.id, "color")],
    };
    const { ir } = graphToIr(graph);
    expect(ir.edges).toEqual([]);
  });

  it("drops a node whose data fails to lower (e.g. a LUT with no name)", () => {
    const lut = node("lut", { name: "" });
    const graph: Graph = { nodes: [lut], edges: [] };
    const { ir, issues } = graphToIr(graph);
    expect(ir.nodes).toEqual([]);
    expect(issues[0]!.reason).toBe("loweringError");
  });
});

// ---- subgraph inlining (#57) ------------------------------------------------

function subgraphNode(id: string, sub: Subgraph): Node {
  return {
    id,
    kind: SUBGRAPH_KIND,
    position: { x: 0, y: 0 },
    data: sub as unknown as Record<string, unknown>,
  };
}

/**
 * Canonical, id-independent signature of an IrGraph: the multiset of node ops
 * plus the multiset of edges expressed as `(sourceOp, sourcePort) →
 * (targetOp, targetPort)`. Two graphs with this same signature are structurally
 * equivalent modulo id renaming (sufficient here: every fixture node has a
 * distinct op, so the op uniquely identifies its endpoints).
 */
function irSignature(ir: IrGraph): { ops: string[]; edges: string[] } {
  const opById = new Map(ir.nodes.map((n) => [n.id, JSON.stringify(n.op)]));
  return {
    ops: ir.nodes.map((n) => JSON.stringify(n.op)).sort(),
    edges: ir.edges
      .map(
        (e) =>
          `${opById.get(e.source.node)}#${e.source.port} -> ${opById.get(e.target.node)}#${e.target.port}`,
      )
      .sort(),
  };
}

describe("graphToIr — subgraph inlining EXIT GATE (collapsed ≡ expanded)", () => {
  // Interior: Texcoord → Sample(Source) → Output, with the Texcoord's `uv` fed
  // from a boundary INPUT and the Sample feeding a boundary OUTPUT. Wrap the
  // Sample in a subgraph; an exterior Texcoord drives the input boundary and an
  // exterior Output reads the output boundary.
  function buildPair(): { expanded: Graph; collapsed: Graph } {
    // The fully-inlined ("expanded") reference graph.
    const tc = node("texcoord");
    const src = node("source");
    const out = node("output");
    const expanded: Graph = {
      nodes: [tc, src, out],
      edges: [
        edge(tc.id, "uv", src.id, "coord"),
        edge(src.id, "out", out.id, "color"),
      ],
    };

    // The collapsed equivalent: the Source sample lives INSIDE a subgraph with a
    // vec2 `coordIn` input boundary (→ the interior sample's coord) and a vec4
    // `colorOut` output boundary (← the interior sample's out).
    const interior = node("source");
    const boundaryPorts: BoundaryPort[] = [
      { name: "coordIn", ty: "vec2", direction: "in", interiorNode: interior.id, interiorPort: "coord" },
      { name: "colorOut", ty: "vec4", direction: "out", interiorNode: interior.id, interiorPort: "out" },
    ];
    const sub: Subgraph = {
      id: "sub-1",
      name: "Sampler",
      nodes: [interior],
      edges: [],
      boundaryPorts,
    };
    const ctc = node("texcoord");
    const cout = node("output");
    const sg = subgraphNode("sg-node", sub);
    const collapsed: Graph = {
      nodes: [ctc, sg, cout],
      edges: [
        edge(ctc.id, "uv", sg.id, "coordIn"),
        edge(sg.id, "colorOut", cout.id, "color"),
      ],
    };
    return { expanded, collapsed };
  }

  it("a collapsed subgraph lowers to the SAME IR (modulo ids) as its inlined form", () => {
    const { expanded, collapsed } = buildPair();
    const irExpanded = graphToIrGraph(expanded);
    const irCollapsed = graphToIrGraph(collapsed);
    // No subgraph op kind ever reaches the IR — every node lowered to a primitive.
    expect(irCollapsed.nodes.length).toBe(irExpanded.nodes.length);
    expect(irSignature(irCollapsed)).toEqual(irSignature(irExpanded));
  });

  it("reports no lowering issues for a collapsed graph (the subgraph node is inlined first)", () => {
    const { collapsed } = buildPair();
    const { issues } = graphToIr(collapsed);
    expect(issues).toEqual([]);
  });
});
