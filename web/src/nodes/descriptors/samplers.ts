// Inputs / Samplers (#49) — the texture-reading boundary nodes. Each lowers to
// `NodeOp::Sample { texture: TextureSource }` and carries the SAME port shape:
//   input  "coord" (vec2, REQUIRED — an unconnected coord is a danglingInput error)
//   output "out"   (vec4)
// The variable part (history depth / pass index / LUT name) lives in `node.data`;
// the inspector (#47) edits it and the pipeline view (#46) reads the binding.
import type { TextureSource } from "../../bindings/TextureSource";
import { readInteger, readString, requireString } from "../data";
import type { InspectorField, NodeData, NodeDescriptor, PortSpec } from "../types";
import { NodeLoweringError } from "../types";

/** Every sampler exposes a required vec2 coord input. */
const SAMPLER_INPUTS: PortSpec[] = [{ name: "coord", type: "vec2", label: "UV" }];
/** Every sampler outputs a vec4 sampled color. */
const SAMPLER_OUTPUTS: PortSpec[] = [{ name: "out", type: "vec4", label: "color" }];

/** Build a sampler descriptor whose `texture` is fixed (Source / Original). */
function fixedSampler(
  kind: string,
  label: string,
  description: string,
  texture: TextureSource,
): NodeDescriptor {
  return {
    kind,
    category: "input",
    label,
    description,
    inputs: () => SAMPLER_INPUTS,
    outputs: () => SAMPLER_OUTPUTS,
    defaultData: () => ({}),
    inspector: () => [],
    toNodeOp: () => ({ kind: "sample", texture }),
  };
}

/** Build a sampler descriptor whose `texture` carries a `u32` index in `data.index`. */
function indexedSampler(
  kind: "originalHistory" | "passOutput" | "passFeedback",
  label: string,
  description: string,
  indexLabel: string,
): NodeDescriptor {
  const inspector: InspectorField[] = [
    { key: "index", label: indexLabel, kind: "integer", min: 0, step: 1 },
  ];
  return {
    kind,
    category: "input",
    label,
    description,
    inputs: () => SAMPLER_INPUTS,
    outputs: () => SAMPLER_OUTPUTS,
    defaultData: () => ({ index: 0 }),
    inspector: () => inspector,
    toNodeOp: (data: NodeData) => {
      const index = readInteger(data, "index", 0);
      // A NEGATIVE index is the dangling/out-of-range sentinel removePass writes
      // (DANGLING_INDEX = -1) when the referenced pass was deleted — see
      // pipeline/passOps.ts. TextureSource.index is a Rust u32, so a negative
      // index cannot round-trip over IPC: instead of silently clamping it to 0
      // (which would re-point the sampler at PassOutput0/PassFeedback0 and
      // mis-wire the chain), we REFUSE to lower it. graphToIr catches this as a
      // node-keyed lowering error so the editor flags the offending node inline
      // and the pipeline is marked invalid (never dispatched to the preview).
      if (index < 0) {
        throw new NodeLoweringError(kind, "samples a removed pass — rewire it");
      }
      return { kind: "sample", texture: { kind, index } };
    },
  };
}

/** Source — the current pass's input texture. */
export const sourceDescriptor = fixedSampler(
  "source",
  "Source",
  "Sample the current pass input texture.",
  { kind: "source" },
);

/** Original — the unfiltered first-pass input (Original). */
export const originalDescriptor = fixedSampler(
  "original",
  "Original",
  "Sample the original (first-pass) input texture.",
  { kind: "original" },
);

/** OriginalHistoryN — a previous original frame, N frames back. */
export const originalHistoryDescriptor = indexedSampler(
  "originalHistory",
  "Original History",
  "Sample a previous original frame (OriginalHistoryN).",
  "Frames back",
);

/** PassOutputN — the output of an earlier pass. */
export const passOutputDescriptor = indexedSampler(
  "passOutput",
  "Pass Output",
  "Sample an earlier pass's output (PassOutputN).",
  "Pass index",
);

/** PassFeedbackN — the previous frame's output of a pass (feedback). */
export const passFeedbackDescriptor = indexedSampler(
  "passFeedback",
  "Pass Feedback",
  "Sample a pass's previous-frame output (PassFeedbackN).",
  "Pass index",
);

/** LUT — a named lookup texture declared in the project's `luts`. */
export const lutDescriptor: NodeDescriptor = {
  kind: "lut",
  category: "input",
  label: "LUT",
  description: "Sample a named lookup texture (LUT).",
  inputs: () => SAMPLER_INPUTS,
  outputs: () => SAMPLER_OUTPUTS,
  defaultData: () => ({ name: "" }),
  inspector: () => [{ key: "name", label: "LUT name", kind: "text" }],
  toNodeOp: (data: NodeData) => ({
    kind: "sample",
    texture: { kind: "lut", name: requireString("lut", data, "name") },
  }),
  toLutName: (data: NodeData) => {
    const name = readString(data, "name", "");
    return name.length > 0 ? name : null;
  },
};

/** All sampler descriptors, registered together. */
export const samplerDescriptors: NodeDescriptor[] = [
  sourceDescriptor,
  originalDescriptor,
  originalHistoryDescriptor,
  passOutputDescriptor,
  passFeedbackDescriptor,
  lutDescriptor,
];
