// Output (#49) — the final color sink. Exactly one reachable Output per graph; it
// has a single required vec4 input "color" and NO output (NodeOp::Output).
import type { NodeDescriptor, PortSpec } from "../types";

/** The Output node's single required vec4 color input. */
const OUTPUT_INPUTS: PortSpec[] = [{ name: "color", type: "vec4", label: "color" }];

/** Output — writes its `color` input to FragColor (the pass's final color). */
export const outputDescriptor: NodeDescriptor = {
  kind: "output",
  category: "output",
  label: "Output",
  description: "The final pass color (FragColor).",
  inputs: () => OUTPUT_INPUTS,
  outputs: () => [],
  defaultData: () => ({}),
  inspector: () => [],
  toNodeOp: () => ({ kind: "output" }),
};
