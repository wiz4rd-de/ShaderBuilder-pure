// Tests for the Math + Vector taxonomy (#50). They assert the descriptor port
// shapes + the lowered NodeOp::Expr, plus two representative arithmetic graphs
// (luma = dot(rgb, weights); a parameterised mix) round-tripped through graphToIr
// into the exact IrGraph shape the Phase-4 emitter consumes.
import { describe, expect, it } from "vitest";

import type { Edge } from "../../bindings/Edge";
import type { Graph } from "../../bindings/Graph";
import type { Node } from "../../bindings/Node";
import { graphToIr } from "../graphToIr";
import { defaultDataFor, getDescriptor, hasDescriptor, requireDescriptor } from "../registry";

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

describe("math descriptor", () => {
  it("registers a single math node in the math category", () => {
    expect(hasDescriptor("math")).toBe(true);
    expect(getDescriptor("math")?.category).toBe("math");
  });

  it("defaults to add: two operands a,b → out", () => {
    const d = requireDescriptor("math");
    const data = d.defaultData();
    expect(d.inputs(data).map((p) => p.name)).toEqual(["a", "b"]);
    expect(d.outputs(data).map((p) => p.name)).toEqual(["out"]);
    expect(d.toNodeOp(data)).toEqual({ kind: "expr", op: { op: "add" }, operands: ["a", "b"] });
  });

  it("lowers each binary arithmetic operator to its ExprOp with operands [a,b]", () => {
    const d = requireDescriptor("math");
    for (const op of ["add", "sub", "mul", "div", "min", "max", "pow"]) {
      expect(d.toNodeOp({ op })).toEqual({ kind: "expr", op: { op }, operands: ["a", "b"] });
      expect(d.inputs({ op }).map((p) => p.name)).toEqual(["a", "b"]);
    }
  });

  it("ternary mix/clamp carry their named operands in order", () => {
    const d = requireDescriptor("math");
    expect(d.toNodeOp({ op: "mix" })).toEqual({
      kind: "expr",
      op: { op: "mix" },
      operands: ["a", "b", "t"],
    });
    expect(d.inputs({ op: "mix" }).map((p) => p.name)).toEqual(["a", "b", "t"]);
    expect(d.toNodeOp({ op: "clamp" })).toEqual({
      kind: "expr",
      op: { op: "clamp" },
      operands: ["x", "lo", "hi"],
    });
    expect(d.inputs({ op: "clamp" }).map((p) => p.name)).toEqual(["x", "lo", "hi"]);
  });

  it("unary math ops take one operand x", () => {
    const d = requireDescriptor("math");
    for (const op of ["abs", "floor", "fract", "sin", "cos", "normalize", "length"]) {
      expect(d.toNodeOp({ op })).toEqual({ kind: "expr", op: { op }, operands: ["x"] });
      expect(d.inputs({ op }).map((p) => p.name)).toEqual(["x"]);
    }
  });

  it("dot is binary and outputs a float; length outputs a float; normalize a vec4", () => {
    const d = requireDescriptor("math");
    expect(d.toNodeOp({ op: "dot" })).toEqual({ kind: "expr", op: { op: "dot" }, operands: ["a", "b"] });
    expect(d.outputs({ op: "dot" }).map((p) => p.type)).toEqual(["float"]);
    expect(d.outputs({ op: "length" }).map((p) => p.type)).toEqual(["float"]);
    expect(d.outputs({ op: "normalize" }).map((p) => p.type)).toEqual(["vec4"]);
  });

  it("an unknown op falls back to add (so the node always lowers)", () => {
    const d = requireDescriptor("math");
    expect(d.toNodeOp({ op: "bogus" })).toEqual({ kind: "expr", op: { op: "add" }, operands: ["a", "b"] });
  });
});

describe("swizzle descriptor", () => {
  it("lowers to Swizzle{mask} with operand [in]", () => {
    const d = requireDescriptor("swizzle");
    expect(d.toNodeOp({ mask: "xyz" })).toEqual({
      kind: "expr",
      op: { op: "swizzle", mask: "xyz" },
      operands: ["in"],
    });
    expect(d.inputs({}).map((p) => p.name)).toEqual(["in"]);
  });

  it("derives the output type from the mask length", () => {
    const d = requireDescriptor("swizzle");
    expect(d.outputs({ mask: "x" }).map((p) => p.type)).toEqual(["float"]);
    expect(d.outputs({ mask: "xy" }).map((p) => p.type)).toEqual(["vec2"]);
    expect(d.outputs({ mask: "rgb" }).map((p) => p.type)).toEqual(["vec3"]);
    expect(d.outputs({ mask: "xyzw" }).map((p) => p.type)).toEqual(["vec4"]);
  });

  it("trims whitespace and defaults an empty mask to xyzw (checker stays authoritative)", () => {
    const d = requireDescriptor("swizzle");
    expect(d.toNodeOp({ mask: "  bgr " })).toEqual({
      kind: "expr",
      op: { op: "swizzle", mask: "bgr" },
      operands: ["in"],
    });
    expect(d.toNodeOp({ mask: "" })).toEqual({
      kind: "expr",
      op: { op: "swizzle", mask: "xyzw" },
      operands: ["in"],
    });
  });

  it("passes a malformed mask through unchanged (a compile_graph diagnostic, not a TS guess)", () => {
    const d = requireDescriptor("swizzle");
    // 'g' belongs to rgba but mixing sets / out-of-range is a CHECKER error — we do
    // not reimplement that here; we simply forward the mask verbatim.
    expect(d.toNodeOp({ mask: "xg" })).toEqual({
      kind: "expr",
      op: { op: "swizzle", mask: "xg" },
      operands: ["in"],
    });
  });
});

describe("split descriptor", () => {
  it("lowers to a single-component Swizzle, output float", () => {
    const d = requireDescriptor("split");
    expect(d.toNodeOp({ component: "y" })).toEqual({
      kind: "expr",
      op: { op: "swizzle", mask: "y" },
      operands: ["in"],
    });
    expect(d.outputs({}).map((p) => p.type)).toEqual(["float"]);
    expect(d.inputs({}).map((p) => p.name)).toEqual(["in"]);
  });

  it("defaults the component to x and rejects an unknown component", () => {
    const d = requireDescriptor("split");
    expect(d.toNodeOp({})).toEqual({ kind: "expr", op: { op: "swizzle", mask: "x" }, operands: ["in"] });
    expect(d.toNodeOp({ component: "nope" })).toEqual({
      kind: "expr",
      op: { op: "swizzle", mask: "x" },
      operands: ["in"],
    });
  });
});

describe("combine descriptor", () => {
  it("constructs vec2/vec3/vec4 with the ordered component operands", () => {
    const d = requireDescriptor("combine");
    expect(d.toNodeOp({ ty: "vec2" })).toEqual({
      kind: "expr",
      op: { op: "construct", ty: "vec2" },
      operands: ["x", "y"],
    });
    expect(d.inputs({ ty: "vec2" }).map((p) => p.name)).toEqual(["x", "y"]);
    expect(d.toNodeOp({ ty: "vec3" })).toEqual({
      kind: "expr",
      op: { op: "construct", ty: "vec3" },
      operands: ["x", "y", "z"],
    });
    expect(d.toNodeOp({ ty: "vec4" })).toEqual({
      kind: "expr",
      op: { op: "construct", ty: "vec4" },
      operands: ["x", "y", "z", "w"],
    });
    expect(d.inputs({ ty: "vec4" }).map((p) => p.name)).toEqual(["x", "y", "z", "w"]);
  });

  it("each component port is a float input; output is the constructed vecN", () => {
    const d = requireDescriptor("combine");
    expect(d.inputs({ ty: "vec3" }).map((p) => p.type)).toEqual(["float", "float", "float"]);
    expect(d.outputs({ ty: "vec3" }).map((p) => p.type)).toEqual(["vec3"]);
  });
});

describe("graphToIr — luma = dot(Source.rgb, weights)", () => {
  // Source → Swizzle(rgb) → dot(a=rgb, b=weights) → Combine(vec4{luma,luma,luma,1})
  // → Output. Asserts the exact IrGraph the Phase-4 emitter turns into the
  // hand-written-equivalent slang.
  it("produces the expected typed IrGraph (Expr ops + PortRef edges)", () => {
    const tc = node("texcoord");
    const src = node("source");
    const sw = node("swizzle", { mask: "rgb" });
    const weights = node("const", { constType: "vec3", value: [0.299, 0.587, 0.114] });
    const dot = node("math", { op: "dot" });
    const one = node("const", { constType: "float", value: 1 });
    const combine = node("combine", { ty: "vec4" });
    const out = node("output");

    const graph: Graph = {
      nodes: [tc, src, sw, weights, dot, one, combine, out],
      edges: [
        edge(tc.id, "uv", src.id, "coord"),
        edge(src.id, "out", sw.id, "in"),
        edge(sw.id, "out", dot.id, "a"),
        edge(weights.id, "out", dot.id, "b"),
        // luma broadcast into the three colour components + opaque alpha
        edge(dot.id, "out", combine.id, "x"),
        edge(dot.id, "out", combine.id, "y"),
        edge(dot.id, "out", combine.id, "z"),
        edge(one.id, "out", combine.id, "w"),
        edge(combine.id, "out", out.id, "color"),
      ],
    };

    const { ir, issues } = graphToIr(graph);
    expect(issues).toEqual([]);

    expect(ir.nodes.find((n) => n.id === sw.id)!.op).toEqual({
      kind: "expr",
      op: { op: "swizzle", mask: "rgb" },
      operands: ["in"],
    });
    expect(ir.nodes.find((n) => n.id === dot.id)!.op).toEqual({
      kind: "expr",
      op: { op: "dot" },
      operands: ["a", "b"],
    });
    expect(ir.nodes.find((n) => n.id === combine.id)!.op).toEqual({
      kind: "expr",
      op: { op: "construct", ty: "vec4" },
      operands: ["x", "y", "z", "w"],
    });

    // Edges keep the operand port names (a / b / x..w) the IR expects.
    expect(ir.edges).toContainEqual({
      source: { node: sw.id, port: "out" },
      target: { node: dot.id, port: "a" },
    });
    expect(ir.edges).toContainEqual({
      source: { node: weights.id, port: "out" },
      target: { node: dot.id, port: "b" },
    });
    expect(ir.edges).toContainEqual({
      source: { node: dot.id, port: "out" },
      target: { node: combine.id, port: "y" },
    });
    expect(ir.edges).toContainEqual({
      source: { node: one.id, port: "out" },
      target: { node: combine.id, port: "w" },
    });
  });
});

describe("graphToIr — parameterised mix(a, b, GAMMA)", () => {
  it("collects the Param + wires the mix t operand", () => {
    const a = node("const", { constType: "vec4", value: [0, 0, 0, 1] });
    const b = node("source");
    const tc = node("texcoord");
    const p = node("param", { name: "BLEND", label: "Blend", default: 0.5, min: 0, max: 1, step: 0.01 });
    const mix = node("math", { op: "mix" });
    const out = node("output");

    const graph: Graph = {
      nodes: [a, b, tc, p, mix, out],
      edges: [
        edge(tc.id, "uv", b.id, "coord"),
        edge(a.id, "out", mix.id, "a"),
        edge(b.id, "out", mix.id, "b"),
        edge(p.id, "out", mix.id, "t"),
        edge(mix.id, "out", out.id, "color"),
      ],
    };

    const { ir, parameters, issues } = graphToIr(graph);
    expect(issues).toEqual([]);
    expect(parameters).toEqual([
      { name: "BLEND", label: "Blend", default: 0.5, min: 0, max: 1, step: 0.01 },
    ]);
    expect(ir.nodes.find((n) => n.id === mix.id)!.op).toEqual({
      kind: "expr",
      op: { op: "mix" },
      operands: ["a", "b", "t"],
    });
    expect(ir.edges).toContainEqual({
      source: { node: p.id, port: "out" },
      target: { node: mix.id, port: "t" },
    });
  });
});
