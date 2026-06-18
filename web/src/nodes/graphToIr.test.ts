import { describe, expect, it } from "vitest";

import type { Edge } from "../bindings/Edge";
import type { Graph } from "../bindings/Graph";
import type { Node } from "../bindings/Node";
import { defaultDataFor } from "./registry";
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
