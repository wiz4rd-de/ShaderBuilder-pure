// Connection type-checking parity + behaviour tests (#65).
//
// The CROSS-LANGUAGE PARITY suite is the load-bearing one: it asserts the TS
// `connectionLegal` predicate reproduces the Rust-generated golden truth table
// (`__goldens__/connectionLegality.json`) row-for-row. The golden is regenerated
// by `crates/ir/tests/connection_parity.rs` from the SAME `connection_legal`
// predicate the type checker delegates to — so a green test here + green Rust
// parity test together prove the editor's drag-time verdict AGREES with the IR.
import { describe, expect, it } from "vitest";

import type { Node } from "../bindings/Node";
import type { PortType } from "../bindings/PortType";
import golden from "./__goldens__/connectionLegality.json";
import {
  type ConnectionTarget,
  classifyTarget,
  connectionLegal,
  edgeIsIllegal,
  judgeConnection,
  sourceOutputType,
  swizzleResult,
} from "./portTypeChecking";

/** One golden row as emitted by the Rust generator. */
interface GoldenRow {
  src: PortType;
  targetKind: "assignable" | "sampleCoord" | "exprOperand";
  targetType: string;
  legal: boolean;
}

/** Reconstruct the ConnectionTarget a golden row encodes. */
function targetOf(row: GoldenRow): ConnectionTarget {
  switch (row.targetKind) {
    case "assignable":
      return { kind: "assignable", type: row.targetType as PortType };
    case "sampleCoord":
      return { kind: "sampleCoord" };
    case "exprOperand":
      return { kind: "exprOperand" };
  }
}

/** A minimal graph node with the given kind + data. */
function node(id: string, kind: string, data: Record<string, unknown> = {}): Node {
  return { id, kind, position: { x: 0, y: 0 }, data };
}

describe("connection-legality parity with the Rust checker", () => {
  const rows = golden as GoldenRow[];

  it("loads the full golden table", () => {
    // 7 src types × (7 assignable + sampleCoord + exprOperand) = 63 rows.
    expect(rows.length).toBe(63);
  });

  it("reproduces every golden row", () => {
    for (const row of rows) {
      expect(connectionLegal(row.src, targetOf(row))).toBe(row.legal);
    }
  });
});

describe("connectionLegal — representative pairs", () => {
  it("permits exact, widen, and broadcast assignments", () => {
    expect(connectionLegal("vec4", { kind: "assignable", type: "vec4" })).toBe(true);
    expect(connectionLegal("int", { kind: "assignable", type: "float" })).toBe(true);
    expect(connectionLegal("float", { kind: "assignable", type: "vec3" })).toBe(true);
  });

  it("refuses incompatible assignments", () => {
    expect(connectionLegal("vec2", { kind: "assignable", type: "vec3" })).toBe(false);
    expect(connectionLegal("sampler2D", { kind: "assignable", type: "float" })).toBe(false);
    expect(connectionLegal("float", { kind: "assignable", type: "int" })).toBe(false);
  });

  it("ties sampler2D outputs to sampler2D inputs only", () => {
    expect(connectionLegal("sampler2D", { kind: "assignable", type: "sampler2D" })).toBe(true);
    expect(connectionLegal("sampler2D", { kind: "assignable", type: "vec4" })).toBe(false);
    expect(connectionLegal("vec4", { kind: "assignable", type: "sampler2D" })).toBe(false);
  });

  it("tightens Sample.coord to an exact vec2 (no float broadcast)", () => {
    expect(connectionLegal("vec2", { kind: "sampleCoord" })).toBe(true);
    expect(connectionLegal("float", { kind: "sampleCoord" })).toBe(false);
    expect(connectionLegal("vec3", { kind: "sampleCoord" })).toBe(false);
  });

  it("treats Expr operands as polymorphic (always structurally legal)", () => {
    for (const t of ["float", "vec2", "vec3", "vec4", "int", "bool", "sampler2D"] as PortType[]) {
      expect(connectionLegal(t, { kind: "exprOperand" })).toBe(true);
    }
  });
});

describe("swizzleResult — downstream type of a swizzle connection", () => {
  it("yields the float type of the mask length", () => {
    expect(swizzleResult("vec4", "rgb")).toBe("vec3");
    expect(swizzleResult("vec4", "x")).toBe("float");
    expect(swizzleResult("vec4", "xy")).toBe("vec2");
    expect(swizzleResult("vec2", "yx")).toBe("vec2");
  });

  it("rejects illegal masks", () => {
    expect(swizzleResult("vec2", "z")).toBeNull();
    expect(swizzleResult("vec4", "xr")).toBeNull(); // mixed accessor sets
    expect(swizzleResult("int", "x")).toBeNull();
    expect(swizzleResult("vec4", "")).toBeNull();
  });
});

describe("classifyTarget — descriptor-driven sink classification", () => {
  it("classifies a sampler coord as the tightened vec2 sink", () => {
    const target = classifyTarget(node("s", "source"), "coord");
    expect(target).toEqual({ kind: "sampleCoord" });
  });

  it("classifies a Math operand as a polymorphic Expr operand", () => {
    const target = classifyTarget(node("m", "math", { op: "add" }), "a");
    expect(target).toEqual({ kind: "exprOperand" });
  });

  it("classifies the Output color sink as assignable vec4", () => {
    const target = classifyTarget(node("o", "output"), "color");
    expect(target).toEqual({ kind: "assignable", type: "vec4" });
  });

  it("reads a CustomSnippet port's CURRENT declared type", () => {
    const snip = node("c", "customSnippet", {
      body: "result = tex;",
      inputs: [{ name: "tex", type: "sampler2D" }],
      outputs: [{ name: "result", type: "vec4" }],
    });
    expect(classifyTarget(snip, "tex")).toEqual({ kind: "assignable", type: "sampler2D" });
  });

  it("returns null for a non-existent port", () => {
    expect(classifyTarget(node("o", "output"), "nope")).toBeNull();
  });
});

describe("sourceOutputType — data-derived output types", () => {
  it("derives a Swizzle output type from its stored mask", () => {
    expect(sourceOutputType(node("z", "swizzle", { mask: "rgb" }), "out")).toBe("vec3");
    expect(sourceOutputType(node("z", "swizzle", { mask: "x" }), "out")).toBe("float");
  });

  it("derives a Combine output type from its target type", () => {
    expect(sourceOutputType(node("k", "combine", { ty: "vec3" }), "out")).toBe("vec3");
  });
});

describe("judgeConnection — the canvas isValidConnection oracle", () => {
  const g = (nodes: Node[]) => ({ nodes, edges: [] });

  it("refuses sampler2D-style vec4 into a tightened coord", () => {
    const graph = g([node("src", "source"), node("dst", "original")]);
    // source.out is vec4 → original.coord (vec2 tightened) — illegal.
    const v = judgeConnection(graph, "src", "out", "dst", "coord");
    expect(v.legal).toBe(false);
  });

  it("permits a vec2 texcoord into a sampler coord (exact)", () => {
    const graph = g([node("uv", "texcoord"), node("s", "source")]);
    const v = judgeConnection(graph, "uv", "out", "s", "coord");
    expect(v.legal).toBe(true);
    expect(v.coercion).toBe("none"); // sampleCoord sink, not an assignable coercion
  });

  it("marks a float → vecN broadcast as coerced", () => {
    // A float Const into an Output(vec4) color sink broadcasts (float → vec4).
    const graph = g([node("f", "const", { constType: "float", value: 1 }), node("o", "output")]);
    const v = judgeConnection(graph, "f", "out", "o", "color");
    expect(v.legal).toBe(true);
    expect(v.coercion).toBe("broadcast");
  });

  it("does not block an edge it cannot resolve", () => {
    const graph = g([node("x", "unknownKind"), node("o", "output")]);
    const v = judgeConnection(graph, "x", "out", "o", "color");
    expect(v.legal).toBe(true);
    expect(v.coercion).toBe("none");
  });
});

describe("edgeIsIllegal — stale-edge re-validation after a data change", () => {
  it("flags an edge made illegal by a later node-data change", () => {
    // A Combine retargeted from vec4 to vec3 now feeds an Output(vec4) color sink.
    const graph = {
      nodes: [node("c", "combine", { ty: "vec3" }), node("o", "output")],
      edges: [{ id: "e1", source: "c", sourcePort: "out", target: "o", targetPort: "color" }],
    };
    expect(edgeIsIllegal(graph, graph.edges[0]!)).toBe(true);
  });

  it("does not flag a still-legal edge", () => {
    const graph = {
      nodes: [node("c", "combine", { ty: "vec4" }), node("o", "output")],
      edges: [{ id: "e1", source: "c", sourcePort: "out", target: "o", targetPort: "color" }],
    };
    expect(edgeIsIllegal(graph, graph.edges[0]!)).toBe(false);
  });

  it("does not flag an unjudgeable edge", () => {
    const graph = {
      nodes: [node("x", "unknownKind"), node("o", "output")],
      edges: [{ id: "e1", source: "x", sourcePort: "out", target: "o", targetPort: "color" }],
    };
    expect(edgeIsIllegal(graph, graph.edges[0]!)).toBe(false);
  });
});
