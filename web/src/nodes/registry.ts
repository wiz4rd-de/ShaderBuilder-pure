// The NODE-DESCRIPTOR REGISTRY (#49) — the single keyed map of every editor node
// `kind` → its descriptor. The palette, the inspector (#47), the canvas node
// components, and graphToIr ALL resolve nodes through here. Later issues EXTEND
// the taxonomy by appending descriptors to `ALL_DESCRIPTORS` (#50 math, #51 color,
// #52 custom snippet) — never by special-casing a kind elsewhere.
import { coordinateDescriptors } from "./descriptors/coordinates";
import { outputDescriptor } from "./descriptors/output";
import { samplerDescriptors } from "./descriptors/samplers";
import { valueDescriptors } from "./descriptors/values";
import type { NodeCategory, NodeData, NodeDescriptor } from "./types";

/**
 * Every registered descriptor, in palette order. #49 lands the boundary
 * categories (inputs/samplers, coordinates/UV, constants/params/builtins, output);
 * #50/#51/#52 push their math/color/custom descriptors onto this list.
 */
export const ALL_DESCRIPTORS: ReadonlyArray<NodeDescriptor> = [
  ...samplerDescriptors,
  ...coordinateDescriptors,
  ...valueDescriptors,
  outputDescriptor,
];

/** The descriptor map, keyed by `kind`. Built once at module load. */
const REGISTRY: ReadonlyMap<string, NodeDescriptor> = new Map(
  ALL_DESCRIPTORS.map((d) => [d.kind, d]),
);

/** Look up a descriptor by node `kind`, or `undefined` when unregistered. */
export function getDescriptor(kind: string): NodeDescriptor | undefined {
  return REGISTRY.get(kind);
}

/** Look up a descriptor by `kind`, throwing when the kind is unregistered. */
export function requireDescriptor(kind: string): NodeDescriptor {
  const d = REGISTRY.get(kind);
  if (!d) {
    throw new Error(`unknown node kind "${kind}" — no registered descriptor`);
  }
  return d;
}

/** Whether a `kind` has a registered descriptor. */
export function hasDescriptor(kind: string): boolean {
  return REGISTRY.has(kind);
}

/** All registered descriptors (palette order). */
export function listDescriptors(): ReadonlyArray<NodeDescriptor> {
  return ALL_DESCRIPTORS;
}

/** All registered descriptors in a given category (palette sectioning). */
export function descriptorsByCategory(category: NodeCategory): NodeDescriptor[] {
  return ALL_DESCRIPTORS.filter((d) => d.category === category);
}

/** The default `data` for a freshly-placed node of `kind` (empty if unknown). */
export function defaultDataFor(kind: string): NodeData {
  return getDescriptor(kind)?.defaultData() ?? {};
}

/** Human-ordered list of categories that currently have at least one descriptor. */
export function nonEmptyCategories(): NodeCategory[] {
  const order: NodeCategory[] = [
    "input",
    "coordinate",
    "constant",
    "parameter",
    "builtin",
    "math",
    "color",
    "custom",
    "output",
  ];
  return order.filter((c) => ALL_DESCRIPTORS.some((d) => d.category === c));
}
