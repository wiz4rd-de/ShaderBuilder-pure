// Sampling helpers (#51) — the spatial nodes the scanlines + bloom exit demo
// leans on: an N-tap gaussian blur, a sharp-bilinear UV snap, and the mask /
// grille generators.
//
// THE SAMPLER CONSTRAINT (read crates/ir/src/lower.rs + typecheck.rs):
// the frozen IR has NO node that produces a `sampler2D` value — samplers are
// opaque bindings created only by a `Sample` node, addressed by its intrinsic
// `texture`. A CustomSnippet therefore CANNOT take a live sampler and call
// `texture()` on it from a graph wiring (there is no temp to bind, and the
// manifest only emits a binding for textures an actual `Sample` node reads). So
// the sampling helpers here operate on VALUES already pulled from the graph:
//
//  * Gaussian blur takes N pre-sampled `vec4` TAP inputs (the author wires N
//    Sample nodes at offset coords) and blends them with gaussian weights baked
//    from `sigma`. It is ONE direction of a SEPARABLE blur — a full gaussian is
//    two passes (H then V) at the pipeline level, each an N-tap node. (Documented
//    on the descriptor; the exit demo's bloom uses a horizontal then a vertical
//    pass.)
//  * Sharp-bilinear is a COORDINATE-domain transform: `uv` (+ the pass's
//    `SourceSize` as a `vec4` input wired from a Builtin SourceSize node) → a
//    snapped `uv` that, sampled with bilinear filtering, gives a crisp
//    nearest-with-AA upscale. It feeds a Sample.coord.
//  * Mask / grille generators take `uv` (+ `OutputSize` as a `vec4` input wired
//    from a Builtin OutputSize node) → a `vec3` mask multiplier. The mask PITCH
//    is computed from `uv * OutputSize.xy`, so the pattern renders at the
//    SIMULATED-viewport scale / integer-scale (acceptance: mask pitch tracks
//    OutputSize). Multiply the result with the image via a Math/Blend node.
//
// Wiring the *Size builtin as an INPUT PORT (rather than reading `params.*Size`
// inside the body) keeps the snippet's manifest dependency explicit: the Builtin
// node is what makes the emitter declare the uniform, and the edge is what proves
// the dependency to the checker.
import { readInteger, readNumber } from "../data";
import type { InspectorField, NodeData, NodeDescriptor, PortSpec } from "../types";

/** Format a number as a GLSL float literal (always with a decimal point). */
function glslFloat(n: number): string {
  if (!Number.isFinite(n)) return "0.0";
  return Number.isInteger(n) ? `${n}.0` : `${n}`;
}

// ---- N-tap gaussian blur (one separable direction) ------------------------

/** Clamp the tap count to an odd value in [3, 15] (a centred symmetric kernel). */
function tapCount(data: NodeData): number {
  let n = readInteger(data, "taps", 5);
  if (n < 3) n = 3;
  if (n > 15) n = 15;
  // Force odd so there is a single centre tap.
  if (n % 2 === 0) n += 1;
  return n;
}

/** Resolve the gaussian sigma (std-dev in taps), clamped to a sane positive range. */
function sigma(data: NodeData): number {
  const s = readNumber(data, "sigma", 1.5);
  return s > 0.01 ? s : 0.01;
}

/**
 * The normalised gaussian weights for `n` taps centred at 0 with std-dev `sigma`.
 * Tap `i` is offset `i - (n-1)/2` from centre. Weights sum to 1 (normalised) so
 * the blur preserves overall brightness.
 */
export function gaussianWeights(n: number, s: number): number[] {
  const half = (n - 1) / 2;
  const raw: number[] = [];
  let sum = 0;
  for (let i = 0; i < n; i++) {
    const x = i - half;
    const w = Math.exp(-(x * x) / (2 * s * s));
    raw.push(w);
    sum += w;
  }
  return raw.map((w) => w / sum);
}

/**
 * Gaussian Blur (N-tap, separable) — blend N pre-sampled colour taps by a
 * gaussian kernel baked from `sigma`. ONE direction of a separable gaussian:
 * for a full blur use a horizontal then a vertical pass (each an N-tap node),
 * wiring `tap_i` to a Sample of the source at the matching offset coord. The
 * weights are normalised so brightness is preserved.
 */
export const gaussianBlurDescriptor: NodeDescriptor = {
  kind: "gaussianBlur",
  category: "color",
  label: "Gaussian Blur (N-tap)",
  description: "Blend N colour taps by a gaussian kernel (one separable direction).",
  inputs: (data) => {
    const n = tapCount(data);
    const ports: PortSpec[] = [];
    for (let i = 0; i < n; i++) {
      ports.push({ name: `tap${i}`, type: "vec4", label: `tap ${i}` });
    }
    return ports;
  },
  outputs: () => [{ name: "result", type: "vec4", label: "color" }],
  defaultData: () => ({ taps: 5, sigma: 1.5 }),
  inspector: (): InspectorField[] => [
    { key: "taps", label: "Tap count (odd, 3–15)", kind: "integer", min: 3, max: 15, step: 2 },
    { key: "sigma", label: "Sigma (std-dev in taps)", kind: "number", min: 0.01, step: 0.1 },
  ],
  toNodeOp: (data: NodeData) => {
    const n = tapCount(data);
    const weights = gaussianWeights(n, sigma(data));
    const inputs: PortSpec[] = [];
    const terms: string[] = [];
    for (let i = 0; i < n; i++) {
      inputs.push({ name: `tap${i}`, type: "vec4" });
      terms.push(`tap${i} * ${glslFloat(weights[i]!)}`);
    }
    return {
      kind: "customSnippet",
      body: `result = ${terms.join(" + ")};`,
      inputs: inputs.map((p) => ({ name: p.name, type: p.type })),
      outputs: [{ name: "result", type: "vec4" }],
    };
  },
};

// ---- Sharp-bilinear UV snap -----------------------------------------------

/**
 * Sharp-Bilinear — snap a UV toward texel centres so a subsequent bilinear
 * Sample yields a crisp, nearest-with-thin-AA upscale (the classic
 * sharp-bilinear filter). Reads the pass `SourceSize` (wire a Builtin SourceSize
 * node into `sourceSize`) to know the source texel grid. `sharpness` ∈ [0,1]
 * blends between pure bilinear (0) and pure nearest (1).
 */
export const sharpBilinearDescriptor: NodeDescriptor = {
  kind: "sharpBilinear",
  category: "coordinate",
  label: "Sharp-Bilinear UV",
  description: "Snap a UV toward texel centres for a crisp bilinear upscale.",
  inputs: () => [
    { name: "uv", type: "vec2", label: "UV" },
    { name: "sourceSize", type: "vec4", label: "SourceSize" },
  ],
  outputs: () => [{ name: "result", type: "vec2", label: "UV" }],
  defaultData: () => ({ sharpness: 1 }),
  inspector: (): InspectorField[] => [
    { key: "sharpness", label: "Sharpness (0–1)", kind: "number", min: 0, max: 1, step: 0.01 },
  ],
  toNodeOp: (data: NodeData) => {
    let sh = readNumber(data, "sharpness", 1);
    if (sh < 0) sh = 0;
    if (sh > 1) sh = 1;
    // texel = uv * SourceSize.xy; the offset of the sample point from the texel
    // CENTRE is compressed toward 0 as `sharpness` rises, then converted back to
    // UV via SourceSize.zw (= 1/size). `region = 0.5 * (1 - sharpness)` is the
    // half-width of the linear-AA ramp around the centre: at sharpness 0 the offset
    // passes through unchanged (pure bilinear); at sharpness 1 the offset collapses
    // to 0, so every sample is held at the texel centre (pure nearest).
    const region = glslFloat(0.5 * (1 - sh));
    const body = [
      "vec2 texel = uv * sourceSize.xy;",
      "vec2 center = floor(texel) + vec2(0.5);",
      "vec2 frac = texel - center;",
      `vec2 snapped = center + clamp(frac, vec2(-${region}), vec2(${region}));`,
      "result = snapped * sourceSize.zw;",
    ].join("\n");
    return {
      kind: "customSnippet",
      body,
      inputs: [
        { name: "uv", type: "vec2" },
        { name: "sourceSize", type: "vec4" },
      ],
      outputs: [{ name: "result", type: "vec2" }],
    };
  },
};

// ---- Mask / grille generators ---------------------------------------------

/** The supported CRT mask layouts. */
const MASK_TYPES: ReadonlyArray<{ value: string; label: string }> = [
  { value: "apertureGrille", label: "Aperture grille (RGB stripes)" },
  { value: "slotMask", label: "Slot mask" },
  { value: "shadowMask", label: "Shadow mask (RGB triads)" },
];

/** Resolve the selected mask type (defaulting to aperture grille). */
function maskType(data: NodeData): string {
  const v = data["mask"];
  return MASK_TYPES.some((m) => m.value === v) ? (v as string) : MASK_TYPES[0]!.value;
}

/** Resolve the mask strength ∈ [0,1] (0 = no mask, 1 = full). */
function maskStrength(data: NodeData): number {
  let s = readNumber(data, "strength", 0.5);
  if (s < 0) s = 0;
  if (s > 1) s = 1;
  return s;
}

/**
 * Build the mask body for a given layout. `px = uv * outputSize.xy` is the
 * device-pixel coordinate (so the mask PITCH tracks OutputSize / integer-scale —
 * one stripe/cell per output pixel). `strength` lerps the mask multiplier toward
 * white (1) so a partial mask dims rather than blacks out.
 */
function maskBody(type: string, strength: number): string {
  const s = glslFloat(strength);
  const head = "vec2 px = uv * outputSize.xy;";
  switch (type) {
    case "slotMask": {
      // RGB columns with a vertical half-cell stagger (the slot offset).
      return [
        head,
        "float col = mod(px.x, 3.0);",
        "float row = mod(floor(px.y / 1.0) + step(1.5, mod(px.x, 6.0)) * 0.0, 2.0);",
        "vec3 m = vec3(step(col, 1.0), step(1.0, col) * step(col, 2.0), step(2.0, col));",
        "float slot = step(0.5, mod(floor(px.y / 2.0) + floor(px.x / 3.0), 2.0));",
        "m *= mix(1.0, slot, 0.5);",
        `result = mix(vec3(1.0), m * 3.0, ${s});`,
      ].join("\n");
    }
    case "shadowMask": {
      // RGB triads on a hex-ish grid: colour by x phase, dim alternate rows.
      return [
        head,
        "float phase = mod(px.x, 3.0);",
        "vec3 m = vec3(step(phase, 1.0), step(1.0, phase) * step(phase, 2.0), step(2.0, phase));",
        "float rowDim = mix(1.0, 0.7, mod(floor(px.y), 2.0));",
        `result = mix(vec3(1.0), m * 3.0 * rowDim, ${s});`,
      ].join("\n");
    }
    case "apertureGrille":
    default: {
      // Vertical RGB stripes, pitch 3 output pixels.
      return [
        head,
        "float phase = mod(px.x, 3.0);",
        "vec3 m = vec3(step(phase, 1.0), step(1.0, phase) * step(phase, 2.0), step(2.0, phase));",
        `result = mix(vec3(1.0), m * 3.0, ${s});`,
      ].join("\n");
    }
  }
}

/**
 * CRT Mask — generate an aperture-grille / slot / shadow-mask RGB multiplier at
 * the SIMULATED-viewport scale. Wire a Builtin OutputSize node into `outputSize`
 * so the mask pitch tracks the integer-scaled output (one stripe/cell per output
 * pixel). Multiply the `vec3` result with the image (a Math/Blend multiply).
 */
export const crtMaskDescriptor: NodeDescriptor = {
  kind: "crtMask",
  category: "color",
  label: "CRT Mask",
  description: "Aperture-grille / slot / shadow mask at the simulated-viewport scale.",
  inputs: () => [
    { name: "uv", type: "vec2", label: "UV" },
    { name: "outputSize", type: "vec4", label: "OutputSize" },
  ],
  outputs: () => [{ name: "result", type: "vec3", label: "mask" }],
  defaultData: () => ({ mask: "apertureGrille", strength: 0.5 }),
  inspector: (): InspectorField[] => [
    {
      key: "mask",
      label: "Mask type",
      kind: "select",
      options: MASK_TYPES.map((m) => ({ value: m.value, label: m.label })),
    },
    { key: "strength", label: "Strength (0–1)", kind: "number", min: 0, max: 1, step: 0.01 },
  ],
  toNodeOp: (data: NodeData) => ({
    kind: "customSnippet",
    body: maskBody(maskType(data), maskStrength(data)),
    inputs: [
      { name: "uv", type: "vec2" },
      { name: "outputSize", type: "vec4" },
    ],
    outputs: [{ name: "result", type: "vec3" }],
  }),
};

/** All sampling-helper descriptors, in palette order. */
export const samplingDescriptors: NodeDescriptor[] = [
  gaussianBlurDescriptor,
  sharpBilinearDescriptor,
  crtMaskDescriptor,
];
