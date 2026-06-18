// Public surface of the node taxonomy (#49): the registry, the graph→IR bridge,
// the descriptor contract types, and the React Flow node component + types map.
// Downstream issues (#46/#47/#50/#51/#52/#54) import from here.
export * from "./types";
export {
  ALL_DESCRIPTORS,
  getDescriptor,
  requireDescriptor,
  hasDescriptor,
  listDescriptors,
  descriptorsByCategory,
  defaultDataFor,
  nonEmptyCategories,
} from "./registry";
export { graphToIr, graphToIrGraph } from "./graphToIr";
export type { GraphToIrResult, GraphToIrIssue } from "./graphToIr";
export { TaxonomyNode } from "./TaxonomyNode";
export type { TaxonomyNodeData } from "./TaxonomyNode";
export { NODE_TYPES } from "./nodeTypes";
