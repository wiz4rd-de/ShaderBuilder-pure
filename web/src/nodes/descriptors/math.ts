// Math (#50) — the pure-arithmetic core: operators (+ − × ÷) and the common GLSL
// intrinsics (mix, clamp, pow, dot, normalize, min/max, abs, floor/fract, sin/cos,
// length). Every Math node lowers to `NodeOp::Expr { op, operands }`; the operand
// PORT NAMES are the entries of `operands` (the order is the op's operand order),
// which the checker (#40) and emitter (#42) agree on. A node's input ports ARE
// those operand names, so graphToIr's edge→PortRef mapping lines up exactly.
//
// Port TYPES here are declarative hints for the canvas handles / inspector only —
// component-wise math is FLOAT-FAMILY with scalar broadcast + Int→Float widen, and
// the Rust CHECKER is authoritative for the real result type (which depends on the
// connected operands). We declare each operand `float` for the broadcastable
// scalar/component ops and `vec4` for the vector-flavoured ones (dot/normalize/
// length), so a freshly-placed node shows sensible handles; a type mismatch still
// surfaces as a `compile_graph` diagnostic, not a silent TS guess.
import type { ExprOp } from "../../bindings/ExprOp";
import type { PortType } from "../../bindings/PortType";
import type { InspectorField, NodeData, NodeDescriptor, PortSpec } from "../types";

/** The fixed-arity intrinsic ExprOps a Math node can carry (no swizzle/construct —
 *  those are Vector nodes). Each entry declares its operand port names (in operand
 *  order), a representative operand PortType for the handles, and the op's result
 *  PortType hint for the single "out" output. */
interface MathOpSpec {
  /** The `data.op` selector value + the ExprOp `op` tag (1:1). */
  op: ExprOp["op"];
  /** Palette/inspector display label. */
  label: string;
  /** The ordered operand port names — these become the IR `operands` + input ports. */
  operands: string[];
  /** The declared type of every operand handle (a hint; the checker is authoritative). */
  operandType: PortType;
  /** The declared type of the "out" handle (a hint). */
  outputType: PortType;
}

/** Every fixed-arity Math op, in palette order. swizzle/construct live in Vector. */
const MATH_OPS: ReadonlyArray<MathOpSpec> = [
  // Binary arithmetic (component-wise, scalar broadcast) — float-family.
  { op: "add", label: "Add (+)", operands: ["a", "b"], operandType: "float", outputType: "float" },
  { op: "sub", label: "Subtract (−)", operands: ["a", "b"], operandType: "float", outputType: "float" },
  { op: "mul", label: "Multiply (×)", operands: ["a", "b"], operandType: "float", outputType: "float" },
  { op: "div", label: "Divide (÷)", operands: ["a", "b"], operandType: "float", outputType: "float" },
  // Other binary math.
  { op: "min", label: "Min", operands: ["a", "b"], operandType: "float", outputType: "float" },
  { op: "max", label: "Max", operands: ["a", "b"], operandType: "float", outputType: "float" },
  { op: "pow", label: "Pow", operands: ["a", "b"], operandType: "float", outputType: "float" },
  // Ternary.
  { op: "mix", label: "Mix (lerp)", operands: ["a", "b", "t"], operandType: "float", outputType: "float" },
  { op: "clamp", label: "Clamp", operands: ["x", "lo", "hi"], operandType: "float", outputType: "float" },
  // Unary math.
  { op: "abs", label: "Abs", operands: ["x"], operandType: "float", outputType: "float" },
  { op: "floor", label: "Floor", operands: ["x"], operandType: "float", outputType: "float" },
  { op: "fract", label: "Fract", operands: ["x"], operandType: "float", outputType: "float" },
  { op: "sin", label: "Sin", operands: ["x"], operandType: "float", outputType: "float" },
  { op: "cos", label: "Cos", operands: ["x"], operandType: "float", outputType: "float" },
  // Vector-flavoured intrinsics (operate on a vector; dot/length → float).
  { op: "dot", label: "Dot", operands: ["a", "b"], operandType: "vec4", outputType: "float" },
  { op: "normalize", label: "Normalize", operands: ["x"], operandType: "vec4", outputType: "vec4" },
  { op: "length", label: "Length", operands: ["x"], operandType: "vec4", outputType: "float" },
];

/** The op-spec map, keyed by selector value. */
const MATH_OP_BY_KEY: ReadonlyMap<string, MathOpSpec> = new Map(MATH_OPS.map((s) => [s.op, s]));

/** Resolve the selected Math op spec from `data.op`, defaulting to `add`. */
function mathSpec(data: NodeData): MathOpSpec {
  const v = data["op"];
  return (typeof v === "string" && MATH_OP_BY_KEY.get(v)) || MATH_OP_BY_KEY.get("add")!;
}

/** The select-field options listing every Math op. */
const MATH_OP_OPTIONS: ReadonlyArray<{ value: string; label: string }> = MATH_OPS.map((s) => ({
  value: s.op,
  label: s.label,
}));

/**
 * Math — one descriptor whose op (and thus its operand ports + arity) is chosen in
 * `data.op`. Its input ports ARE the op's operand names so the canvas handles + the
 * graphToIr edges address the exact port names the IR `operands` declares.
 */
export const mathDescriptor: NodeDescriptor = {
  kind: "math",
  category: "math",
  label: "Math",
  description: "Arithmetic operator / GLSL intrinsic (→ Expr).",
  inputs: (data) => {
    const spec = mathSpec(data);
    return spec.operands.map<PortSpec>((name) => ({ name, type: spec.operandType, label: name }));
  },
  outputs: (data) => [{ name: "out", type: mathSpec(data).outputType, label: "out" }],
  defaultData: () => ({ op: "add" }),
  inspector: (): InspectorField[] => [
    { key: "op", label: "Operation", kind: "select", options: MATH_OP_OPTIONS },
  ],
  toNodeOp: (data) => {
    const spec = mathSpec(data);
    return {
      kind: "expr",
      op: { op: spec.op } as ExprOp,
      operands: [...spec.operands],
    };
  },
};

/** All Math descriptors (currently the single op-selecting node). */
export const mathDescriptors: NodeDescriptor[] = [mathDescriptor];
