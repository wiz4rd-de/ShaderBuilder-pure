// Vector (#50) — swizzle / split / combine, the component-layout nodes.
//
//  * Swizzle → `ExprOp::Swizzle { mask }`, operand `["in"]`. The mask (xyzw / rgba /
//              stpq, length 1..4) selects + reorders components; the output type is
//              the mask length (1 → float, 2 → vec2, …). Swizzle is FLOAT-FAMILY only.
//  * Split   → a single-component swizzle helper (`ExprOp::Swizzle { mask: <comp> }`,
//              operand `["in"]`, output `float`) with the component chosen by a
//              friendly select instead of a raw mask. N split nodes = a full split.
//  * Combine → `ExprOp::Construct { ty }`, operands the component ports IN ORDER
//              (x, y[, z[, w]]) — builds a vecN from scalars (Float→component).
//
// Mask validity (one accessor set, length 1..4, in range) is NOT re-checked here —
// the Rust CHECKER is authoritative (a malformed mask → a `compile_graph`
// diagnostic). We only normalise whitespace + supply a sane default so a fresh node
// lowers, and derive the output handle type from the mask length for the canvas.
import type { ExprOp } from "../../bindings/ExprOp";
import type { PortType } from "../../bindings/PortType";
import { readString } from "../data";
import type { InspectorField, NodeData, NodeDescriptor, PortSpec } from "../types";

// ---- Swizzle --------------------------------------------------------------

/** Read + normalise the swizzle mask from `data.mask` (trimmed; default "xyzw"). */
function swizzleMask(data: NodeData): string {
  const raw = readString(data, "mask", "xyzw").trim();
  return raw.length > 0 ? raw : "xyzw";
}

/**
 * The vecN/float PortType a mask of length 1..4 yields — a **canvas handle hint
 * ONLY**, never the legality verdict. The EDGE-LEGALITY source-output type is
 * computed input-aware in `portTypeChecking.sourceOutputType` via
 * `swizzleResult(inputType, mask)` (mirroring `PortType::swizzle_result`): a mask
 * length alone cannot tell whether the swizzle is legal for the connected input
 * (e.g. `.xyz` of a vec2), so deriving the verdict from length would FALSE-BLOCK
 * wires the IR accepts. This hint just gives the handle a colour before an input
 * is connected.
 */
function maskOutputType(mask: string): PortType {
  switch (mask.length) {
    case 1:
      return "float";
    case 2:
      return "vec2";
    case 3:
      return "vec3";
    case 4:
      return "vec4";
    default:
      // Out-of-range length — the checker rejects it; show vec4 so a handle exists.
      return "vec4";
  }
}

/** Swizzle — select/reorder components via a mask (xyzw / rgba / stpq). */
export const swizzleDescriptor: NodeDescriptor = {
  kind: "swizzle",
  category: "math",
  label: "Swizzle",
  description: "Select / reorder vector components by mask (e.g. xyz, bgr, x).",
  inputs: () => [{ name: "in", type: "vec4", label: "in" }],
  outputs: (data) => [{ name: "out", type: maskOutputType(swizzleMask(data)), label: "out" }],
  defaultData: () => ({ mask: "xyzw" }),
  inspector: (): InspectorField[] => [
    { key: "mask", label: "Mask (xyzw / rgba / stpq)", kind: "text" },
  ],
  toNodeOp: (data) => ({
    kind: "expr",
    op: { op: "swizzle", mask: swizzleMask(data) } as ExprOp,
    operands: ["in"],
  }),
};

// ---- Split ----------------------------------------------------------------

/** The single-component swizzle options a Split node offers. */
const SPLIT_COMPONENTS: ReadonlyArray<{ value: string; label: string }> = [
  { value: "x", label: "X (r / s)" },
  { value: "y", label: "Y (g / t)" },
  { value: "z", label: "Z (b / p)" },
  { value: "w", label: "W (a / q)" },
];

/** Resolve the selected component mask (single accessor), defaulting to "x". */
function splitComponent(data: NodeData): string {
  const v = readString(data, "component", "x");
  return SPLIT_COMPONENTS.some((c) => c.value === v) ? v : "x";
}

/**
 * Split — extract ONE component of a vector as a float (a friendly single-component
 * swizzle). Add one Split per component to fully split a vecN.
 */
export const splitDescriptor: NodeDescriptor = {
  kind: "split",
  category: "math",
  label: "Split",
  description: "Extract one component of a vector as a float.",
  inputs: () => [{ name: "in", type: "vec4", label: "in" }],
  outputs: () => [{ name: "out", type: "float", label: "out" }],
  defaultData: () => ({ component: "x" }),
  inspector: (): InspectorField[] => [
    { key: "component", label: "Component", kind: "select", options: SPLIT_COMPONENTS },
  ],
  toNodeOp: (data) => ({
    kind: "expr",
    op: { op: "swizzle", mask: splitComponent(data) } as ExprOp,
    operands: ["in"],
  }),
};

// ---- Combine --------------------------------------------------------------

/** The vector type a Combine node can construct + its ordered component ports. */
const COMBINE_TYPES: ReadonlyArray<{ value: PortType; label: string; operands: string[] }> = [
  { value: "vec2", label: "Vec2", operands: ["x", "y"] },
  { value: "vec3", label: "Vec3", operands: ["x", "y", "z"] },
  { value: "vec4", label: "Vec4", operands: ["x", "y", "z", "w"] },
];

/** Resolve the selected Combine target type (defaulting to vec4). */
function combineType(data: NodeData): (typeof COMBINE_TYPES)[number] {
  const v = readString(data, "ty", "vec4");
  return COMBINE_TYPES.find((t) => t.value === v) ?? COMBINE_TYPES[2]!;
}

/**
 * Combine — build a vecN from scalar components in order (Construct{ty}). The
 * operand ports are the component names (x, y[, z[, w]]); each is a float input.
 */
export const combineDescriptor: NodeDescriptor = {
  kind: "combine",
  category: "math",
  label: "Combine",
  description: "Construct a vector from scalar components (vec2 / vec3 / vec4).",
  inputs: (data) =>
    combineType(data).operands.map<PortSpec>((name) => ({ name, type: "float", label: name })),
  outputs: (data) => [{ name: "out", type: combineType(data).value, label: "out" }],
  defaultData: () => ({ ty: "vec4" }),
  inspector: (): InspectorField[] => [
    {
      key: "ty",
      label: "Type",
      kind: "select",
      options: COMBINE_TYPES.map((t) => ({ value: t.value, label: t.label })),
    },
  ],
  toNodeOp: (data) => {
    const spec = combineType(data);
    return {
      kind: "expr",
      op: { op: "construct", ty: spec.value } as ExprOp,
      operands: [...spec.operands],
    };
  },
};

/** All Vector descriptors (swizzle + split + combine). */
export const vectorDescriptors: NodeDescriptor[] = [
  swizzleDescriptor,
  splitDescriptor,
  combineDescriptor,
];
