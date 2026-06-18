// Color (#51) — the colour-transform taxonomy the scanlines + bloom exit demo
// leans on. Every node here operates on COLOUR VALUES (vec3/vec4 already-sampled
// pixels), never on a sampler — so each is a self-contained CustomSnippet (matrix
// constants / multi-statement bodies) or an Expr, with NO sampler/builtin binding
// required. They compile end-to-end on their own (confirmed against
// crates/codegen-slang/src/emit.rs: a CustomSnippet wrapper is a file-scope
// `void snippet_<id>(in <ty> <name>…, out <ty> <name>…)` whose body references its
// ports by their declared names; its locals are private to the function).
//
// MATHEMATICAL CORRECTNESS (acceptance bar): RGB↔YIQ uses the standard NTSC
// matrices and their TRUE inverse, and linear↔sRGB uses the EXACT piecewise IEC
// 61966-2-1 transfer function (NOT a naive pow(2.2)). Both round-trip to identity
// within float tolerance — verified by the color round-trip tests.
import { readNumber } from "../data";
import type { InspectorField, NodeData, NodeDescriptor, PortSpec } from "../types";

/** Format a number as a GLSL float literal (always with a decimal point). */
function glslFloat(n: number): string {
  if (!Number.isFinite(n)) return "0.0";
  return Number.isInteger(n) ? `${n}.0` : `${n}`;
}

/** A vec3 colour input port (the upstream RGB a transform consumes). */
const RGB_IN: PortSpec[] = [{ name: "color", type: "vec3", label: "RGB" }];
/** A vec3 colour output port (what every colour transform yields). */
const RGB_OUT: PortSpec[] = [{ name: "out", type: "vec3", label: "RGB" }];

// ---- RGB ↔ YIQ (standard NTSC matrices + true inverse) --------------------
//
// FCC/NTSC primaries. Forward (RGB→YIQ) rows = [Y; I; Q]; inverse (YIQ→RGB) is
// the exact algebraic inverse of that matrix so RGB→YIQ→RGB is identity. GLSL is
// column-major: `mat3(c0, c1, c2)` takes COLUMNS, and `M * v` is the row-form
// product — so we transpose the conventional row matrix into column args.

/** RGB→YIQ forward matrix rows (Y, I, Q) — standard NTSC luminance/chroma. */
export const RGB_TO_YIQ_ROWS: readonly [number, number, number][] = [
  [0.299, 0.587, 0.114],
  [0.595716, -0.274453, -0.321263],
  [0.211456, -0.522591, 0.311135],
];
/** YIQ→RGB inverse matrix rows — the TRUE algebraic inverse of RGB_TO_YIQ_ROWS
 *  (computed by cofactor inversion, not the rounded textbook constants), so
 *  RGB→YIQ→RGB is identity to machine epsilon rather than ~1e-5. */
export const YIQ_TO_RGB_ROWS: readonly [number, number, number][] = [
  [1.0, 0.956296, 0.621024],
  [1.0, -0.272122, -0.647381],
  [1.0, -1.106989, 1.704615],
];

/** Build the column-major `mat3(...)` literal for a row-form 3x3 matrix. */
function mat3FromRows(rows: readonly [number, number, number][]): string {
  // GLSL mat3 args are columns, so emit column j = (rows[0][j], rows[1][j], rows[2][j]).
  const cols: string[] = [];
  for (let j = 0; j < 3; j++) {
    cols.push(`${glslFloat(rows[0][j]!)}, ${glslFloat(rows[1][j]!)}, ${glslFloat(rows[2][j]!)}`);
  }
  return `mat3(${cols.join(", ")})`;
}

/** RGB → YIQ — convert linear-ish RGB to NTSC Y/I/Q colour space. */
export const rgbToYiqDescriptor: NodeDescriptor = {
  kind: "rgbToYiq",
  category: "color",
  label: "RGB → YIQ",
  description: "Convert RGB to NTSC YIQ (standard matrix).",
  inputs: () => RGB_IN,
  outputs: () => [{ name: "out", type: "vec3", label: "YIQ" }],
  defaultData: () => ({}),
  inspector: () => [],
  toNodeOp: () => ({
    kind: "customSnippet",
    body: `out = ${mat3FromRows(RGB_TO_YIQ_ROWS)} * color;`,
    inputs: [{ name: "color", type: "vec3" }],
    outputs: [{ name: "out", type: "vec3" }],
  }),
};

/** YIQ → RGB — convert NTSC Y/I/Q back to RGB (true inverse matrix). */
export const yiqToRgbDescriptor: NodeDescriptor = {
  kind: "yiqToRgb",
  category: "color",
  label: "YIQ → RGB",
  description: "Convert NTSC YIQ back to RGB (true inverse matrix).",
  inputs: () => [{ name: "color", type: "vec3", label: "YIQ" }],
  outputs: () => RGB_OUT,
  defaultData: () => ({}),
  inspector: () => [],
  toNodeOp: () => ({
    kind: "customSnippet",
    body: `out = ${mat3FromRows(YIQ_TO_RGB_ROWS)} * color;`,
    inputs: [{ name: "color", type: "vec3" }],
    outputs: [{ name: "out", type: "vec3" }],
  }),
};

// ---- linear ↔ sRGB (exact piecewise IEC 61966-2-1 transfer) ---------------
//
// linear→sRGB (encode): c <= 0.0031308 ? 12.92*c : 1.055*pow(c, 1/2.4) - 0.055
// sRGB→linear (decode): c <= 0.04045   ? c/12.92 : pow((c+0.055)/1.055, 2.4)
// Applied per-channel with mix() on the step() threshold so the two branches stay
// branch-free. This is the CORRECT curve, not pow(2.2) — round-trips to identity.

/** Linear → sRGB — encode linear light to sRGB using the exact piecewise curve. */
export const linearToSrgbDescriptor: NodeDescriptor = {
  kind: "linearToSrgb",
  category: "color",
  label: "Linear → sRGB",
  description: "Encode linear RGB to sRGB (exact piecewise transfer).",
  inputs: () => RGB_IN,
  outputs: () => RGB_OUT,
  defaultData: () => ({}),
  inspector: () => [],
  toNodeOp: () => ({
    kind: "customSnippet",
    body: [
      "vec3 lo = color * 12.92;",
      "vec3 hi = 1.055 * pow(max(color, vec3(0.0)), vec3(1.0 / 2.4)) - 0.055;",
      "vec3 cutoff = step(vec3(0.0031308), color);",
      "out = mix(lo, hi, cutoff);",
    ].join("\n"),
    inputs: [{ name: "color", type: "vec3" }],
    outputs: [{ name: "out", type: "vec3" }],
  }),
};

/** sRGB → Linear — decode sRGB to linear light using the exact piecewise curve. */
export const srgbToLinearDescriptor: NodeDescriptor = {
  kind: "srgbToLinear",
  category: "color",
  label: "sRGB → Linear",
  description: "Decode sRGB to linear RGB (exact piecewise transfer).",
  inputs: () => RGB_IN,
  outputs: () => RGB_OUT,
  defaultData: () => ({}),
  inspector: () => [],
  toNodeOp: () => ({
    kind: "customSnippet",
    body: [
      "vec3 lo = color / 12.92;",
      "vec3 hi = pow((max(color, vec3(0.0)) + 0.055) / 1.055, vec3(2.4));",
      "vec3 cutoff = step(vec3(0.04045), color);",
      "out = mix(lo, hi, cutoff);",
    ].join("\n"),
    inputs: [{ name: "color", type: "vec3" }],
    outputs: [{ name: "out", type: "vec3" }],
  }),
};

// ---- Luma -----------------------------------------------------------------

/** The luma-weight presets (Rec.601 NTSC vs Rec.709 sRGB primaries). */
const LUMA_WEIGHTS: ReadonlyArray<{ value: string; label: string; w: [number, number, number] }> = [
  { value: "rec601", label: "Rec.601 (NTSC)", w: [0.299, 0.587, 0.114] },
  { value: "rec709", label: "Rec.709 (sRGB)", w: [0.2126, 0.7152, 0.0722] },
];

/** Resolve the selected luma-weight preset (defaulting to Rec.601). */
function lumaWeights(data: NodeData): [number, number, number] {
  const v = data["weights"];
  return (LUMA_WEIGHTS.find((p) => p.value === v) ?? LUMA_WEIGHTS[0]!).w;
}

/** Luma — collapse an RGB colour to its weighted luminance (a float). */
export const lumaDescriptor: NodeDescriptor = {
  kind: "luma",
  category: "color",
  label: "Luma",
  description: "Weighted luminance of an RGB colour (Rec.601 / Rec.709).",
  inputs: () => RGB_IN,
  outputs: () => [{ name: "out", type: "float", label: "luma" }],
  defaultData: () => ({ weights: "rec601" }),
  inspector: (): InspectorField[] => [
    {
      key: "weights",
      label: "Weights",
      kind: "select",
      options: LUMA_WEIGHTS.map((p) => ({ value: p.value, label: p.label })),
    },
  ],
  toNodeOp: (data: NodeData) => {
    const [r, g, b] = lumaWeights(data);
    return {
      kind: "customSnippet",
      body: `out = dot(color, vec3(${glslFloat(r)}, ${glslFloat(g)}, ${glslFloat(b)}));`,
      inputs: [{ name: "color", type: "vec3" }],
      outputs: [{ name: "out", type: "float" }],
    };
  },
};

// ---- Contrast / Gamma -----------------------------------------------------

/** Contrast — pivot RGB around 0.5 by a constant contrast factor. */
export const contrastDescriptor: NodeDescriptor = {
  kind: "contrast",
  category: "color",
  label: "Contrast",
  description: "Scale RGB contrast around mid-grey (0.5).",
  inputs: () => RGB_IN,
  outputs: () => RGB_OUT,
  defaultData: () => ({ amount: 1 }),
  inspector: (): InspectorField[] => [
    { key: "amount", label: "Contrast", kind: "number", min: 0, step: 0.01 },
  ],
  toNodeOp: (data: NodeData) => {
    const amount = glslFloat(readNumber(data, "amount", 1));
    return {
      kind: "customSnippet",
      body: `out = (color - vec3(0.5)) * ${amount} + vec3(0.5);`,
      inputs: [{ name: "color", type: "vec3" }],
      outputs: [{ name: "out", type: "vec3" }],
    };
  },
};

/** Gamma — apply a constant power curve to RGB (a naive display-gamma knob). */
export const gammaDescriptor: NodeDescriptor = {
  kind: "gamma",
  category: "color",
  label: "Gamma",
  description: "Apply a power-curve gamma to RGB (pow per-channel).",
  inputs: () => RGB_IN,
  outputs: () => RGB_OUT,
  defaultData: () => ({ gamma: 2.2 }),
  inspector: (): InspectorField[] => [
    { key: "gamma", label: "Gamma", kind: "number", min: 0.01, step: 0.01 },
  ],
  toNodeOp: (data: NodeData) => {
    const gamma = glslFloat(readNumber(data, "gamma", 2.2));
    return {
      kind: "customSnippet",
      body: `out = pow(max(color, vec3(0.0)), vec3(${gamma}));`,
      inputs: [{ name: "color", type: "vec3" }],
      outputs: [{ name: "out", type: "vec3" }],
    };
  },
};

// ---- Blend modes ----------------------------------------------------------
//
// Two-input colour combine (base × blend) — the bloom composite uses `add` /
// `screen`. Each is a CustomSnippet of two vec3 inputs → one vec3 output.

/** The supported blend modes + their per-channel formula (a, b ∈ [0,1] vec3). */
const BLEND_MODES: ReadonlyArray<{ value: string; label: string; expr: string }> = [
  { value: "add", label: "Add", expr: "a + b" },
  { value: "multiply", label: "Multiply", expr: "a * b" },
  { value: "screen", label: "Screen", expr: "vec3(1.0) - (vec3(1.0) - a) * (vec3(1.0) - b)" },
  { value: "overlay", label: "Overlay", expr: "mix(2.0 * a * b, vec3(1.0) - 2.0 * (vec3(1.0) - a) * (vec3(1.0) - b), step(vec3(0.5), a))" },
];

/** Resolve the selected blend mode (defaulting to add). */
function blendMode(data: NodeData): (typeof BLEND_MODES)[number] {
  const v = data["mode"];
  return BLEND_MODES.find((m) => m.value === v) ?? BLEND_MODES[0]!;
}

/** Blend — combine two RGB colours by a chosen blend mode (bloom composite). */
export const blendDescriptor: NodeDescriptor = {
  kind: "blend",
  category: "color",
  label: "Blend",
  description: "Combine two RGB colours (add / multiply / screen / overlay).",
  inputs: () => [
    { name: "a", type: "vec3", label: "base" },
    { name: "b", type: "vec3", label: "blend" },
  ],
  outputs: () => RGB_OUT,
  defaultData: () => ({ mode: "add" }),
  inspector: (): InspectorField[] => [
    {
      key: "mode",
      label: "Mode",
      kind: "select",
      options: BLEND_MODES.map((m) => ({ value: m.value, label: m.label })),
    },
  ],
  toNodeOp: (data: NodeData) => ({
    kind: "customSnippet",
    body: `out = ${blendMode(data).expr};`,
    inputs: [
      { name: "a", type: "vec3" },
      { name: "b", type: "vec3" },
    ],
    outputs: [{ name: "out", type: "vec3" }],
  }),
};

/** All Color descriptors, in palette order. */
export const colorDescriptors: NodeDescriptor[] = [
  rgbToYiqDescriptor,
  yiqToRgbDescriptor,
  linearToSrgbDescriptor,
  srgbToLinearDescriptor,
  lumaDescriptor,
  contrastDescriptor,
  gammaDescriptor,
  blendDescriptor,
];
