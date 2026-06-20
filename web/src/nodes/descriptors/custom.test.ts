// Tests for the custom-snippet node (#52). They assert:
//  * the descriptor is registered in the `custom` category with editable ports,
//  * default data lowers to a well-formed CustomSnippet whose PortDecls match the
//    descriptor's declared ports,
//  * edited ports (rename/retype/add) round-trip through `setPorts` → data →
//    inputs/outputs and are carried into the lowered op (a sampler2D input too),
//  * the cheap `unresolvedSnippetPorts` pre-check flags body/port mismatches, and
//  * a Source → snippet → Output graph round-trips through graphToIr with its
//    edges addressing the snippet's typed ports.
import { describe, expect, it } from "vitest";

import type { Edge } from "../../bindings/Edge";
import type { Graph } from "../../bindings/Graph";
import type { Node } from "../../bindings/Node";
import { graphToIr } from "../graphToIr";
import { defaultDataFor, getDescriptor } from "../registry";
import type { PortSignature } from "../types";
import { customSnippetDescriptor, unresolvedSnippetPorts } from "./custom";

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

describe("custom-snippet descriptor", () => {
  it("is registered in the custom category with editable ports", () => {
    const d = getDescriptor("customSnippet");
    expect(d).toBe(customSnippetDescriptor);
    expect(d?.category).toBe("custom");
    expect(d?.editablePorts).toBeTruthy();
  });

  it("default data lowers to a CustomSnippet whose ports match the descriptor", () => {
    const data = customSnippetDescriptor.defaultData();
    const op = customSnippetDescriptor.toNodeOp(data);
    expect(op.kind).toBe("customSnippet");
    if (op.kind !== "customSnippet") return;
    expect(op.inputs).toEqual(customSnippetDescriptor.inputs(data).map((p) => ({ name: p.name, type: p.type })));
    expect(op.outputs).toEqual(customSnippetDescriptor.outputs(data).map((p) => ({ name: p.name, type: p.type })));
    // The default body assigns its single output and reads its single input.
    expect(op.body).toContain("result");
    expect(op.body).toContain("color");
  });

  it("carries edited ports (incl. a sampler2D input) into the lowered op", () => {
    const signature: PortSignature = {
      inputs: [
        { name: "tex", type: "sampler2D" },
        { name: "uv", type: "vec2" },
      ],
      outputs: [{ name: "rgba", type: "vec4" }],
    };
    const patch = customSnippetDescriptor.editablePorts!.setPorts({}, signature);
    const data = { ...customSnippetDescriptor.defaultData(), ...patch, body: "rgba = texture(tex, uv);" };
    // The descriptor resolves the same edited signature back out of `data`.
    expect(customSnippetDescriptor.inputs(data).map((p) => `${p.name}:${p.type}`)).toEqual([
      "tex:sampler2D",
      "uv:vec2",
    ]);
    const op = customSnippetDescriptor.toNodeOp(data);
    if (op.kind !== "customSnippet") throw new Error("expected snippet");
    expect(op.inputs).toEqual([
      { name: "tex", type: "sampler2D" },
      { name: "uv", type: "vec2" },
    ]);
    expect(op.outputs).toEqual([{ name: "rgba", type: "vec4" }]);
  });

  it("throws when a snippet declares no output", () => {
    const data = { ...customSnippetDescriptor.defaultData(), outputs: [] };
    expect(() => customSnippetDescriptor.toNodeOp(data)).toThrow();
  });
});

describe("unresolvedSnippetPorts pre-check", () => {
  it("returns no unresolved ports when every declared name appears in the body", () => {
    const sig: PortSignature = {
      inputs: [{ name: "color", type: "vec4" }],
      outputs: [{ name: "result", type: "vec4" }],
    };
    expect(unresolvedSnippetPorts("result = color * 2.0;", sig)).toEqual([]);
  });

  it("flags a port the body never references", () => {
    const sig: PortSignature = {
      inputs: [{ name: "color", type: "vec4" }],
      outputs: [{ name: "result", type: "vec4" }],
    };
    // Body assigns `result` but never reads `color` (a likely typo).
    expect(unresolvedSnippetPorts("result = vec4(1.0);", sig)).toEqual(["color"]);
  });

  it("does not count a name that appears only inside a comment", () => {
    const sig: PortSignature = {
      inputs: [{ name: "color", type: "vec4" }],
      outputs: [{ name: "result", type: "vec4" }],
    };
    const body = "// color is unused here\nresult = vec4(0.0);";
    expect(unresolvedSnippetPorts(body, sig)).toEqual(["color"]);
  });
});

describe("graphToIr round-trips a snippet graph", () => {
  it("Source → snippet → Output lowers with the snippet's typed ports wired", () => {
    const uv = node("texcoord");
    const src = node("source");
    const snip = node("customSnippet", {
      body: "result = clamp(color, vec4(0.0), vec4(1.0));",
      inputs: [{ name: "color", type: "vec4" }],
      outputs: [{ name: "result", type: "vec4" }],
    });
    const out = node("output");
    const graph: Graph = {
      nodes: [uv, src, snip, out],
      edges: [
        edge(uv.id, "uv", src.id, "coord"),
        edge(src.id, "out", snip.id, "color"),
        edge(snip.id, "result", out.id, "color"),
      ],
    };
    const { ir, issues } = graphToIr(graph);
    expect(issues).toEqual([]);
    const lowered = ir.nodes.find((n) => n.id === snip.id);
    expect(lowered?.op.kind).toBe("customSnippet");
    // Edges address the snippet's declared port names.
    expect(ir.edges).toContainEqual({
      source: { node: src.id, port: "out" },
      target: { node: snip.id, port: "color" },
    });
    expect(ir.edges).toContainEqual({
      source: { node: snip.id, port: "result" },
      target: { node: out.id, port: "color" },
    });
  });
});
