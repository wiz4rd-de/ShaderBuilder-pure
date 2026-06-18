// Coordinates / UV (#49) — the nodes that PRODUCE the vec2 that feeds Sample.coord.
//
// KEY DESIGN POINT: Sample.coord is a REQUIRED input (unconnected = danglingInput),
// there is NO Builtin texcoord semantic, and the IR is FROZEN. The clean way to
// expose the raw fragment UV is a CustomSnippet whose body reads `vTexCoord` — which
// IS in scope at fragment-stage file scope (the emitter declares
// `layout(location = 0) in vec2 vTexCoord;` there, and snippet wrapper functions are
// emitted at that same file scope, so the body can read it). Confirmed by reading
// crates/codegen-slang/src/emit.rs (emit_fragment_stage + emit_snippet_fn).
//
// The base "Texcoord" node is that snippet (no inputs → output {uv: vec2}). The
// offset / rotate / warp / curvature transforms are CustomSnippets taking an input
// `uv: vec2` and producing `uv: vec2`, with the transform constants baked into the
// body from `node.data` (re-lowered when the inspector edits them).
import { readNumber } from "../data";
import type { NodeData, NodeDescriptor, PortSpec } from "../types";

/** A vec2 UV input port (the upstream coordinate a transform consumes). */
const UV_IN: PortSpec[] = [{ name: "uv", type: "vec2", label: "UV" }];
/** A vec2 UV output port (what every coordinate node yields). */
const UV_OUT: PortSpec[] = [{ name: "uv", type: "vec2", label: "UV" }];

/** Format a number as a GLSL float literal (always with a decimal point). */
function glslFloat(n: number): string {
  return Number.isInteger(n) ? `${n}.0` : `${n}`;
}

/** Texcoord — the raw fragment UV, read from the in-scope `vTexCoord` global. */
export const texcoordDescriptor: NodeDescriptor = {
  kind: "texcoord",
  category: "coordinate",
  label: "Texcoord",
  description: "The standard fragment UV (vTexCoord).",
  inputs: () => [],
  outputs: () => UV_OUT,
  defaultData: () => ({}),
  inspector: () => [],
  toNodeOp: () => ({
    kind: "customSnippet",
    body: "uv = vTexCoord;",
    inputs: [],
    outputs: [{ name: "uv", type: "vec2" }],
  }),
};

/** Offset — translate a UV by a constant (x, y). */
export const uvOffsetDescriptor: NodeDescriptor = {
  kind: "uvOffset",
  category: "coordinate",
  label: "UV Offset",
  description: "Translate a UV by a constant offset.",
  inputs: () => UV_IN,
  outputs: () => UV_OUT,
  defaultData: () => ({ x: 0, y: 0 }),
  inspector: () => [
    { key: "x", label: "Offset X", kind: "number", step: 0.001 },
    { key: "y", label: "Offset Y", kind: "number", step: 0.001 },
  ],
  toNodeOp: (data: NodeData) => {
    const x = glslFloat(readNumber(data, "x", 0));
    const y = glslFloat(readNumber(data, "y", 0));
    return {
      kind: "customSnippet",
      body: `out_uv = in_uv + vec2(${x}, ${y});`,
      inputs: [{ name: "in_uv", type: "vec2" }],
      outputs: [{ name: "out_uv", type: "vec2" }],
    };
  },
};

/** Rotate — rotate a UV around (0.5, 0.5) by a constant angle (radians). */
export const uvRotateDescriptor: NodeDescriptor = {
  kind: "uvRotate",
  category: "coordinate",
  label: "UV Rotate",
  description: "Rotate a UV around its center by a fixed angle.",
  inputs: () => UV_IN,
  outputs: () => UV_OUT,
  defaultData: () => ({ angle: 0 }),
  inspector: () => [{ key: "angle", label: "Angle (rad)", kind: "number", step: 0.01 }],
  toNodeOp: (data: NodeData) => {
    const angle = glslFloat(readNumber(data, "angle", 0));
    // Rotate about the texture center so the image spins in place.
    const body = [
      `float a = ${angle};`,
      "vec2 c = in_uv - vec2(0.5, 0.5);",
      "vec2 r = vec2(c.x * cos(a) - c.y * sin(a), c.x * sin(a) + c.y * cos(a));",
      "out_uv = r + vec2(0.5, 0.5);",
    ].join("\n");
    return {
      kind: "customSnippet",
      body,
      inputs: [{ name: "in_uv", type: "vec2" }],
      outputs: [{ name: "out_uv", type: "vec2" }],
    };
  },
};

/** Warp — barrel-distort a UV by a constant strength (a simple radial warp). */
export const uvWarpDescriptor: NodeDescriptor = {
  kind: "uvWarp",
  category: "coordinate",
  label: "UV Warp",
  description: "Radially warp a UV around its center (barrel distortion).",
  inputs: () => UV_IN,
  outputs: () => UV_OUT,
  defaultData: () => ({ strength: 0 }),
  inspector: () => [{ key: "strength", label: "Strength", kind: "number", step: 0.01 }],
  toNodeOp: (data: NodeData) => {
    const strength = glslFloat(readNumber(data, "strength", 0));
    const body = [
      `float k = ${strength};`,
      "vec2 c = in_uv - vec2(0.5, 0.5);",
      "float r2 = dot(c, c);",
      "out_uv = (c * (1.0 + k * r2)) + vec2(0.5, 0.5);",
    ].join("\n");
    return {
      kind: "customSnippet",
      body,
      inputs: [{ name: "in_uv", type: "vec2" }],
      outputs: [{ name: "out_uv", type: "vec2" }],
    };
  },
};

/** Curvature — CRT-style screen curvature applied to a UV (constant amount). */
export const uvCurvatureDescriptor: NodeDescriptor = {
  kind: "uvCurvature",
  category: "coordinate",
  label: "UV Curvature",
  description: "Apply CRT-style screen curvature to a UV.",
  inputs: () => UV_IN,
  outputs: () => UV_OUT,
  defaultData: () => ({ amount: 0 }),
  inspector: () => [{ key: "amount", label: "Amount", kind: "number", step: 0.01 }],
  toNodeOp: (data: NodeData) => {
    const amount = glslFloat(readNumber(data, "amount", 0));
    // Bend the UV toward the screen edges proportionally to the orthogonal axis.
    const body = [
      `float amt = ${amount};`,
      "vec2 cc = in_uv * 2.0 - 1.0;",
      "cc.x *= 1.0 + (cc.y * cc.y) * amt;",
      "cc.y *= 1.0 + (cc.x * cc.x) * amt;",
      "out_uv = cc * 0.5 + 0.5;",
    ].join("\n");
    return {
      kind: "customSnippet",
      body,
      inputs: [{ name: "in_uv", type: "vec2" }],
      outputs: [{ name: "out_uv", type: "vec2" }],
    };
  },
};

/** All coordinate/UV descriptors, registered together. */
export const coordinateDescriptors: NodeDescriptor[] = [
  texcoordDescriptor,
  uvOffsetDescriptor,
  uvRotateDescriptor,
  uvWarpDescriptor,
  uvCurvatureDescriptor,
];
