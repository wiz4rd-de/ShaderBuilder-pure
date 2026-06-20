// Constants / Parameters / Builtins (#49) — the leaf value-producing nodes (no
// inputs, one "out" output whose type is the value's type).
//
//  * Const  → NodeOp::Const  (data carries the ConstValue variant + components).
//  * Param  → NodeOp::Param{name} AND contributes a pass-level Parameter (the
//             checker errors `unknownParam` unless the pass declares it).
//  * Builtin→ NodeOp::Builtin{semantic} for each reserved RetroArch semantic.
import type { BuiltinSemantic } from "../../bindings/BuiltinSemantic";
import type { ConstValue } from "../../bindings/ConstValue";
import type { Parameter } from "../../bindings/Parameter";
import type { PortType } from "../../bindings/PortType";
import { readNumber, readNumberTuple, readString, requireString } from "../data";
import type { InspectorField, NodeData, NodeDescriptor, PortSpec } from "../types";

/** A node's single typed "out" output port. */
function out(type: PortType): PortSpec[] {
  return [{ name: "out", type, label: "out" }];
}

// ---- Const ----------------------------------------------------------------

/** The const variants, keyed by the `data.constType` selector value. */
const CONST_TYPES: ReadonlyArray<{ value: ConstValue["kind"]; label: string; port: PortType }> = [
  { value: "float", label: "Float", port: "float" },
  { value: "vec2", label: "Vec2", port: "vec2" },
  { value: "vec3", label: "Vec3", port: "vec3" },
  { value: "vec4", label: "Vec4", port: "vec4" },
  { value: "int", label: "Int", port: "int" },
  { value: "bool", label: "Bool", port: "bool" },
];

/** Resolve the selected const variant (defaulting to float). */
function constKind(data: NodeData): ConstValue["kind"] {
  const v = readString(data, "constType", "float");
  return CONST_TYPES.some((t) => t.value === v) ? (v as ConstValue["kind"]) : "float";
}

/** The output PortType for the selected const variant. */
function constPortType(data: NodeData): PortType {
  const kind = constKind(data);
  return CONST_TYPES.find((t) => t.value === kind)!.port;
}

/** Build the ConstValue from `data` for the selected variant. */
function constValue(data: NodeData): ConstValue {
  switch (constKind(data)) {
    case "vec2":
      return { kind: "vec2", value: readNumberTuple(data, "value", [0, 0]) as [number, number] };
    case "vec3":
      return {
        kind: "vec3",
        value: readNumberTuple(data, "value", [0, 0, 0]) as [number, number, number],
      };
    case "vec4":
      return {
        kind: "vec4",
        value: readNumberTuple(data, "value", [0, 0, 0, 0]) as [number, number, number, number],
      };
    case "int":
      return { kind: "int", value: Math.trunc(readNumber(data, "value", 0)) };
    case "bool":
      return { kind: "bool", value: data["value"] === true };
    case "float":
    default:
      return { kind: "float", value: readNumber(data, "value", 0) };
  }
}

/** The inspector value-field widget for the selected const variant. */
function constValueField(data: NodeData): InspectorField {
  switch (constKind(data)) {
    case "vec2":
      return { key: "value", label: "Value", kind: "vec2" };
    case "vec3":
      return { key: "value", label: "Value", kind: "vec3" };
    case "vec4":
      return { key: "value", label: "Value", kind: "vec4" };
    case "int":
      return { key: "value", label: "Value", kind: "integer" };
    case "bool":
      return { key: "value", label: "Value", kind: "boolean" };
    case "float":
    default:
      return { key: "value", label: "Value", kind: "number", step: 0.01 };
  }
}

/** Const — a typed literal whose variant + components live in `data`. */
export const constDescriptor: NodeDescriptor = {
  kind: "const",
  category: "constant",
  label: "Const",
  description: "A typed literal value.",
  inputs: () => [],
  outputs: (data) => out(constPortType(data)),
  defaultData: () => ({ constType: "float", value: 0 }),
  inspector: (data) => [
    {
      key: "constType",
      label: "Type",
      kind: "select",
      options: CONST_TYPES.map((t) => ({ value: t.value, label: t.label })),
    },
    constValueField(data),
  ],
  toNodeOp: (data) => ({ kind: "const", value: constValue(data) }),
};

// ---- Param ----------------------------------------------------------------

/** The default Parameter authoring values a fresh Param node carries. */
function paramFromData(data: NodeData): Parameter {
  const name = readString(data, "name", "");
  return {
    name,
    label: readString(data, "label", name),
    default: readNumber(data, "default", 0),
    min: readNumber(data, "min", 0),
    max: readNumber(data, "max", 1),
    step: readNumber(data, "step", 0.01),
  };
}

/**
 * Param — a `#pragma parameter` knob. The React node id is NOT the parameter
 * name; `NodeOp::Param.name` is the `data.name` pragma id. It also contributes the
 * pass Parameter so the checker sees it declared (and #53 renders the slider).
 */
export const paramDescriptor: NodeDescriptor = {
  kind: "param",
  category: "parameter",
  label: "Parameter",
  description: "A #pragma parameter runtime knob (float).",
  inputs: () => [],
  outputs: () => out("float"),
  defaultData: () => ({ name: "", label: "", default: 0, min: 0, max: 1, step: 0.01 }),
  inspector: () => [
    { key: "name", label: "Name (pragma id)", kind: "text" },
    { key: "label", label: "Label", kind: "text" },
    { key: "default", label: "Default", kind: "number", step: 0.01 },
    { key: "min", label: "Min", kind: "number", step: 0.01 },
    { key: "max", label: "Max", kind: "number", step: 0.01 },
    { key: "step", label: "Step", kind: "number", step: 0.001 },
  ],
  toNodeOp: (data) => ({ kind: "param", name: requireString("param", data, "name") }),
  toParameter: (data) => {
    const name = readString(data, "name", "");
    return name.length > 0 ? paramFromData(data) : null;
  },
};

// ---- Builtins -------------------------------------------------------------

/** The output PortType each builtin semantic yields (Spec §8.1). */
const BUILTIN_PORT_TYPE: Record<BuiltinSemantic, PortType> = {
  sourceSize: "vec4",
  originalSize: "vec4",
  outputSize: "vec4",
  finalViewportSize: "vec4",
  frameCount: "int",
  frameDirection: "int",
  // MVP is a mat4 — not a port type the taxonomy traffics in (no "out" handle).
  mvp: "vec4",
};

/** A builtin-uniform node for one reserved RetroArch semantic. */
function builtinDescriptor(
  semantic: Exclude<BuiltinSemantic, "mvp">,
  label: string,
  description: string,
): NodeDescriptor {
  return {
    kind: `builtin.${semantic}`,
    category: "builtin",
    label,
    description,
    inputs: () => [],
    outputs: () => out(BUILTIN_PORT_TYPE[semantic]),
    defaultData: () => ({}),
    inspector: () => [],
    toNodeOp: () => ({ kind: "builtin", semantic }),
  };
}

/** All builtin-semantic descriptors. MVP is omitted — it has no value port the
 *  fragment graph consumes (it is the vertex-stage transform). */
export const builtinDescriptors: NodeDescriptor[] = [
  builtinDescriptor("sourceSize", "Source Size", "The Source texture size (xy = px, zw = 1/px)."),
  builtinDescriptor("originalSize", "Original Size", "The Original texture size."),
  builtinDescriptor("outputSize", "Output Size", "This pass's output size."),
  builtinDescriptor(
    "finalViewportSize",
    "Final Viewport Size",
    "The final on-screen viewport size.",
  ),
  builtinDescriptor("frameCount", "Frame Count", "The running frame counter (int)."),
  builtinDescriptor("frameDirection", "Frame Direction", "Playback direction: +1 / -1 (int)."),
];

/** All value-producing descriptors (const + param + builtins). */
export const valueDescriptors: NodeDescriptor[] = [
  constDescriptor,
  paramDescriptor,
  ...builtinDescriptors,
];
