// React Flow `nodeTypes` map (#49): every registered descriptor kind renders with
// the single TaxonomyNode component. Built once from the registry so the canvas
// passes it straight to <ReactFlow nodeTypes={NODE_TYPES} />. A new descriptor is
// picked up automatically — no per-kind component wiring.
import type { NodeTypes } from "@xyflow/react";

import { listDescriptors } from "./registry";
import { TaxonomyNode } from "./TaxonomyNode";

/** kind → component, for every registered descriptor. */
export const NODE_TYPES: NodeTypes = Object.fromEntries(
  listDescriptors().map((d) => [d.kind, TaxonomyNode]),
);
