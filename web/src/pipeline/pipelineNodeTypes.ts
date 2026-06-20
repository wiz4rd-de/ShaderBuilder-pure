// React Flow `nodeTypes` for the PIPELINE view (#46): every pipeline node renders
// with the single PipelineNode card. Kept separate from the per-pass NODE_TYPES
// (nodes/nodeTypes.ts) so the two levels never share a registry.
import type { NodeTypes } from "@xyflow/react";

import { PipelineNode } from "./PipelineNode";

/** The pipeline view registers exactly one node type: a pass card. */
export const PIPELINE_NODE_TYPES: NodeTypes = {
  pipelinePass: PipelineNode,
};
