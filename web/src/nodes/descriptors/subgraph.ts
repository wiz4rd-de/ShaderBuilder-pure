// Subgraph (#57) — the collapsed-subgraph node. Its free-form `data` IS a
// serialized `Subgraph` (#56): a named group of interior nodes/edges with typed
// boundary ports. The node's exterior ports are DERIVED from those boundary
// ports (an `in` port → an input handle, an `out` port → an output handle), each
// carrying the boundary's declared `PortType` so it type-checks identically to a
// primitive port.
//
// A subgraph node NEVER lowers directly: `graphToIr` inlines it (replacing it
// with its interior, rewiring boundary ports) BEFORE the per-node lowering loop
// runs, so `toNodeOp` must never actually be reached. It throws a clear
// NodeLoweringError as a guard — if it ever fires, the inlining step was skipped.
import type { BoundaryPort } from "../../bindings/BoundaryPort";
import type { Subgraph } from "../../bindings/Subgraph";
import { readSubgraph } from "../subgraph";
import type { InspectorField, NodeData, NodeDescriptor, PortSpec } from "../types";
import { NodeLoweringError } from "../types";

/** The `data` key the collapsed node stores its display name under. */
const NAME_KEY = "name";

/** Read the typed `Subgraph` out of a collapsed node's free-form `data`. */
function readSubgraphData(data: NodeData): Subgraph {
  // `readSubgraph` expects a Node; reuse its field-by-field coercion by feeding a
  // minimal node-shaped wrapper so a malformed/partial `data` degrades cleanly.
  return readSubgraph({ id: "", kind: "subgraph", position: { x: 0, y: 0 }, data });
}

/** The boundary ports of a given direction, as canvas PortSpecs (name + type). */
function portsForDirection(
  boundaryPorts: BoundaryPort[],
  direction: BoundaryPort["direction"],
): PortSpec[] {
  return boundaryPorts
    .filter((bp) => bp.direction === direction)
    .map((bp) => ({ name: bp.name, type: bp.ty }));
}

/** An empty subgraph body — the default for a freshly-placed (rare) blank node. */
function emptySubgraph(): Subgraph {
  return { id: "", name: "Subgraph", nodes: [], edges: [], boundaryPorts: [] };
}

/**
 * The collapsed-subgraph descriptor. Ports come from `data.boundaryPorts`; the
 * inspector exposes an editable `name` (renaming the collapsed node); lowering
 * is a guard that must never run (the inlining step removes the node first).
 */
export const subgraphDescriptor: NodeDescriptor = {
  kind: "subgraph",
  // No dedicated palette category — a subgraph is created by collapsing a
  // selection, never placed from the palette. Reuse "custom" for its accent.
  category: "custom",
  label: "Subgraph",
  description: "A collapsed group of nodes exposed through typed boundary ports.",
  // The canvas + breadcrumb show the subgraph's editable `name` (not the static
  // label) so renaming via the inspector updates the collapsed node label.
  title: (data) => readSubgraphData(data).name,
  inputs: (data) => portsForDirection(readSubgraphData(data).boundaryPorts, "in"),
  outputs: (data) => portsForDirection(readSubgraphData(data).boundaryPorts, "out"),
  defaultData: () => emptySubgraph() as unknown as NodeData,
  inspector: (): InspectorField[] => [
    { key: NAME_KEY, label: "Name", kind: "text" },
  ],
  toNodeOp: () => {
    throw new NodeLoweringError(
      "subgraph",
      "subgraph nodes must be inlined before lowering (graphToIr.expandSubgraphs)",
    );
  },
};

/** All subgraph (#57) descriptors, in palette order. */
export const subgraphDescriptors: NodeDescriptor[] = [subgraphDescriptor];
