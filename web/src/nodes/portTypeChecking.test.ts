// Connection type-checking parity + behaviour tests (#65).
//
// The CROSS-LANGUAGE PARITY suite is the load-bearing one: it asserts the TS
// `connectionLegal` predicate reproduces the Rust-generated golden truth table
// (`__goldens__/connectionLegality.json`) row-for-row. The golden is regenerated
// by `crates/ir/tests/connection_parity.rs` from the SAME `connection_legal`
// predicate the type checker delegates to — so a green test here + green Rust
// parity test together prove the editor's drag-time verdict AGREES with the IR.
import { describe, expect, it } from "vitest";

import type { Edge } from "../bindings/Edge";
import type { Node } from "../bindings/Node";
import type { PortType } from "../bindings/PortType";
import golden from "./__goldens__/connectionLegality.json";
import scenarioGolden from "./__goldens__/connectionParityScenarios.json";
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

/** A minimal edge `source.sourcePort → target.targetPort`. */
function edge(
  id: string,
  source: string,
  sourcePort: string,
  target: string,
  targetPort: string,
): Edge {
  return { id, source, sourcePort, target, targetPort };
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

// ---------------------------------------------------------------------------
// Parity #3 (#65 F2/F3): the EDITOR path for DATA-DERIVED sources must agree with
// the IR. The abstract-pair golden above is blind to `sourceOutputType` /
// `classifyTarget` — it only feeds concrete PortType pairs into `connectionLegal`.
// This suite drives `judgeConnection` over real editor Graphs (Swizzle/Split/
// Combine over each input type, into Output.color / Sample.coord / a CustomSnippet
// port) and asserts its `legal` verdict reproduces the Rust scenario golden, which
// was generated from the FULL `check` type checker on the LOWERED IrGraph. Before
// the F2 fix this FAILS (the guard derived a Swizzle output from its mask length,
// false-blocking wires the IR accepts).
// ---------------------------------------------------------------------------

interface ScenarioRow {
  sourceKind: "swizzle" | "split" | "combine";
  mask: string;
  ty: string;
  inputType: PortType;
  sinkKind: "outputColor" | "sampleCoord" | "snippetFloat" | "snippetVec2" | "snippetVec3" | "snippetVec4";
  sinkType: string;
  legal: boolean;
}

/** A const node producing `inputType` on its `out` port. */
function constOf(id: string, inputType: PortType): Node {
  return node(id, "const", { constType: inputType, value: 0 });
}

/** Build the source node + the edges feeding its input(s); returns its id. */
function buildSource(row: ScenarioRow): { nodes: Node[]; edges: Edge[]; sourceId: string } {
  if (row.sourceKind === "combine") {
    // Combine's component ports are floats; its output is its `ty` (input-blind).
    const operands = { vec2: ["x", "y"], vec3: ["x", "y", "z"], vec4: ["x", "y", "z", "w"] }[
      row.ty
    ]!;
    const src = node("src", "combine", { ty: row.ty });
    const consts = operands.map((_p, i) => constOf(`c${i}`, "float"));
    const edges = operands.map((p, i) => edge(`fe${i}`, `c${i}`, "out", "src", p));
    return { nodes: [src, ...consts], edges, sourceId: "src" };
  }
  // swizzle / split — a single `in` operand fed a Const of `inputType`.
  const data =
    row.sourceKind === "split" ? { component: row.mask } : { mask: row.mask };
  const src = node("src", row.sourceKind, data);
  const input = constOf("in_node", row.inputType);
  return {
    nodes: [src, input],
    edges: [edge("ie", "in_node", "out", "src", "in")],
    sourceId: "src",
  };
}

/** Build the sink node + the (source → sink) edge endpoints; returns sink id+port. */
function buildSink(row: ScenarioRow): { nodes: Node[]; sinkId: string; sinkPort: string } {
  switch (row.sinkKind) {
    case "outputColor":
      return { nodes: [node("sink", "output")], sinkId: "sink", sinkPort: "color" };
    case "sampleCoord":
      return { nodes: [node("sink", "source")], sinkId: "sink", sinkPort: "coord" };
    default: {
      const sink = node("sink", "customSnippet", {
        body: "result = vec4(0.0);",
        inputs: [{ name: "in", type: row.sinkType }],
        outputs: [{ name: "result", type: "vec4" }],
      });
      return { nodes: [sink], sinkId: "sink", sinkPort: "in" };
    }
  }
}

describe("data-derived connection parity (Swizzle / Split / Combine) with the IR", () => {
  const scenarios = scenarioGolden as ScenarioRow[];

  it("loads the full scenario golden", () => {
    expect(scenarios.length).toBeGreaterThan(0);
  });

  it("judgeConnection reproduces every scenario row's legal verdict", () => {
    for (const row of scenarios) {
      const source = buildSource(row);
      const sink = buildSink(row);
      const graph = { nodes: [...source.nodes, ...sink.nodes], edges: source.edges };
      const verdict = judgeConnection(graph, source.sourceId, "out", sink.sinkId, sink.sinkPort);
      expect(
        verdict.legal,
        `${row.sourceKind}(mask=${row.mask} ty=${row.ty}) input=${row.inputType} → ${row.sinkKind}`,
      ).toBe(row.legal);
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
  // A Swizzle's output type is now INPUT-AWARE: it is `swizzleResult(inputType,
  // mask)` of the live input, not the mask length (the F2 fix). Wire a vec4 source
  // into the swizzle's `in` operand so the input type is resolvable.
  const swizzleGraph = (mask: string) => ({
    nodes: [node("src", "source"), node("z", "swizzle", { mask })],
    edges: [edge("e", "src", "out", "z", "in")],
  });

  it("derives a Swizzle output type from its mask applied to the live input", () => {
    const g3 = swizzleGraph("rgb");
    expect(sourceOutputType(g3, g3.nodes[1]!, "out")).toBe("vec3");
    const g1 = swizzleGraph("x");
    expect(sourceOutputType(g1, g1.nodes[1]!, "out")).toBe("float");
  });

  it("defers (null) a Swizzle whose input is unconnected", () => {
    // No edge into `in` — the IR infers `None` and skips the downstream edge, so
    // the editor must NOT fabricate a mask-length type.
    const g = { nodes: [node("z", "swizzle", { mask: "rgb" })], edges: [] };
    expect(sourceOutputType(g, g.nodes[0]!, "out")).toBeNull();
  });

  it("defers (null) a Swizzle whose mask is illegal for the live input", () => {
    // A vec2 input cannot be swizzled with `.xyz`; the IR returns None (only an
    // illegalSwizzle on the node, no downstream typeMismatch). The editor defers.
    const g = {
      nodes: [node("k", "combine", { ty: "vec2" }), node("z", "swizzle", { mask: "xyz" })],
      edges: [edge("e", "k", "out", "z", "in")],
    };
    expect(sourceOutputType(g, g.nodes[1]!, "out")).toBeNull();
  });

  it("derives a Split output type as the live input's component (float)", () => {
    const g = {
      nodes: [node("src", "source"), node("s", "split", { component: "y" })],
      edges: [edge("e", "src", "out", "s", "in")],
    };
    expect(sourceOutputType(g, g.nodes[1]!, "out")).toBe("float");
  });

  it("derives a Combine output type from its target type (input-independent)", () => {
    const g = { nodes: [node("k", "combine", { ty: "vec3" })], edges: [] };
    expect(sourceOutputType(g, g.nodes[0]!, "out")).toBe("vec3");
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

  // --- F2 regression: a Swizzle whose output the IR leaves INDETERMINATE must
  // not be FALSE-BLOCKED by a mask-length-fabricated source type. -------------
  it("defers a default Swizzle (unconnected input) dropped into Sample.coord", () => {
    // The IR infers the swizzle output as None (input unfed) and SKIPS the coord
    // edge — no typeMismatch. The editor must DEFER, not block on a fabricated vec4.
    const graph = {
      nodes: [node("z", "swizzle", { mask: "xyzw" }), node("s", "source")],
      edges: [],
    };
    const v = judgeConnection(graph, "z", "out", "s", "coord");
    expect(v.legal).toBe(true);
    expect(v.sourceType).toBeNull();
  });

  it("permits a Swizzle that resolves to vec2 into Sample.coord", () => {
    // src(vec4) → swizzle(.xy) yields a real vec2 → coord accepts it (exact).
    const graph = {
      nodes: [
        node("src", "source"),
        node("z", "swizzle", { mask: "xy" }),
        node("s", "source"),
      ],
      edges: [edge("e", "src", "out", "z", "in")],
    };
    const v = judgeConnection(graph, "z", "out", "s", "coord");
    expect(v.legal).toBe(true);
    expect(v.sourceType).toBe("vec2");
  });

  it("blocks a Swizzle that resolves to vec4 into Sample.coord", () => {
    // src(vec4) → swizzle(.xyzw) is a real vec4 → coord rejects it (not vec2).
    const graph = {
      nodes: [
        node("src", "source"),
        node("z", "swizzle", { mask: "xyzw" }),
        node("s", "source"),
      ],
      edges: [edge("e", "src", "out", "z", "in")],
    };
    const v = judgeConnection(graph, "z", "out", "s", "coord");
    expect(v.legal).toBe(false);
    expect(v.sourceType).toBe("vec4");
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
