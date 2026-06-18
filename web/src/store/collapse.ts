// Pure collapse/expand transforms (#57), factored out of the store so the
// boundary-port derivation + id rewiring is unit-testable without React/zustand.
//
// collapseSelection: turn a multi-selection into ONE kind=="subgraph" node.
//   * The BOUNDARY is every edge with exactly one endpoint in the selection.
//   * Each crossing edge becomes one BoundaryPort: direction `in` when the
//     in-selection endpoint is the edge TARGET (data flows into the selection),
//     `out` when it is the SOURCE. The port `ty` is the PortType of that interior
//     port (read from the interior node's descriptor inputs()/outputs()).
//   * The new node's `data` IS the built Subgraph (interior = selected nodes +
//     the edges fully inside the selection). Crossing parent edges are rewired
//     onto the new node's boundary-port names.
//
// expandSubgraph: the inverse — replace a subgraph node with its interior
// (fresh ids), reconnecting boundary ports to the exterior endpoints on the
// parent edges.
import type { BoundaryPort } from "../bindings/BoundaryPort";
import type { Edge } from "../bindings/Edge";
import type { Graph } from "../bindings/Graph";
import type { Node } from "../bindings/Node";
import type { PortDirection } from "../bindings/PortDirection";
import type { PortType } from "../bindings/PortType";
import type { Subgraph } from "../bindings/Subgraph";
import { getDescriptor } from "../nodes/registry";
import {
  expandSubgraphNode as spliceSubgraph,
  isSubgraphNode,
  readSubgraph,
  SUBGRAPH_KIND,
  type MintId,
} from "../nodes/subgraph";

/** Resolve the declared PortType of `(node, port)` on the given side. */
function portTypeOf(
  node: Node | undefined,
  port: string,
  side: "input" | "output",
): PortType {
  if (!node) {
    return "vec4";
  }
  const descriptor = getDescriptor(node.kind);
  if (!descriptor) {
    return "vec4";
  }
  const ports = side === "input" ? descriptor.inputs(node.data) : descriptor.outputs(node.data);
  return ports.find((p) => p.name === port)?.type ?? "vec4";
}

/** The result of a collapse: the rewritten graph + the id of the new node. */
export interface CollapseResult {
  graph: Graph;
  /** The id of the freshly-created kind=="subgraph" node. */
  subgraphNodeId: string;
}

/**
 * Collapse `nodeIds` (a selection) in `graph` into a single subgraph node named
 * `name`. `mintId` mints fresh ids for the subgraph body id and the wrapper
 * node. Returns the new graph + the wrapper node id, or `null` when the
 * selection is empty / references no real nodes.
 */
export function collapseSelection(
  graph: Graph,
  nodeIds: string[],
  name: string,
  mintId: MintId,
): CollapseResult | null {
  const selected = new Set(nodeIds);
  const interiorNodes = graph.nodes.filter((n) => selected.has(n.id));
  if (interiorNodes.length === 0) {
    return null;
  }
  const nodeById = new Map(graph.nodes.map((n) => [n.id, n] as const));

  // Edges fully inside the selection become the subgraph's interior edges;
  // edges with exactly one endpoint inside are the boundary (one port each).
  const interiorEdges: Edge[] = [];
  const boundaryPorts: BoundaryPort[] = [];
  // Rewire map: original parent edge id -> the rewired exterior edge.
  const rewiredEdges: Edge[] = [];
  const usedNames = new Set<string>();

  /** Mint a unique boundary-port name from a base (e.g. the interior port). */
  function uniquePortName(base: string): string {
    let candidate = base.length > 0 ? base : "port";
    let i = 1;
    while (usedNames.has(candidate)) {
      i += 1;
      candidate = `${base}_${i}`;
    }
    usedNames.add(candidate);
    return candidate;
  }

  const subgraphNodeId = mintId("node");

  for (const edge of graph.edges) {
    const srcIn = selected.has(edge.source);
    const tgtIn = selected.has(edge.target);
    if (srcIn && tgtIn) {
      interiorEdges.push(edge);
      continue;
    }
    if (!srcIn && !tgtIn) {
      continue; // wholly exterior — untouched
    }
    // Crossing edge → one boundary port.
    if (tgtIn) {
      // Data flows INTO the selection: an INPUT boundary at the target port.
      const portName = uniquePortName(edge.targetPort);
      boundaryPorts.push({
        name: portName,
        ty: portTypeOf(nodeById.get(edge.target), edge.targetPort, "input"),
        direction: "in" as PortDirection,
        interiorNode: edge.target,
        interiorPort: edge.targetPort,
      });
      // Redirect the parent edge to the new node's boundary port.
      rewiredEdges.push({ ...edge, target: subgraphNodeId, targetPort: portName });
    } else {
      // Data flows OUT of the selection: an OUTPUT boundary at the source port.
      const portName = uniquePortName(edge.sourcePort);
      boundaryPorts.push({
        name: portName,
        ty: portTypeOf(nodeById.get(edge.source), edge.sourcePort, "output"),
        direction: "out" as PortDirection,
        interiorNode: edge.source,
        interiorPort: edge.sourcePort,
      });
      rewiredEdges.push({ ...edge, source: subgraphNodeId, sourcePort: portName });
    }
  }

  const subgraph: Subgraph = {
    id: mintId("subgraph"),
    name,
    nodes: interiorNodes,
    edges: interiorEdges,
    boundaryPorts,
  };

  const subgraphNode: Node = {
    id: subgraphNodeId,
    kind: SUBGRAPH_KIND,
    position: averagePosition(interiorNodes),
    data: subgraph as unknown as Record<string, unknown>,
  };

  // The surviving exterior nodes (selection removed) + the new wrapper node.
  const nextNodes = graph.nodes.filter((n) => !selected.has(n.id));
  nextNodes.push(subgraphNode);

  // Exterior edges untouched + the rewired crossing edges. Interior edges drop.
  const interiorEdgeIds = new Set(interiorEdges.map((e) => e.id));
  const rewiredById = new Map(rewiredEdges.map((e) => [e.id, e] as const));
  const nextEdges: Edge[] = [];
  for (const edge of graph.edges) {
    if (interiorEdgeIds.has(edge.id)) {
      continue; // moved into the subgraph body
    }
    nextEdges.push(rewiredById.get(edge.id) ?? edge);
  }

  return {
    graph: { nodes: nextNodes, edges: nextEdges },
    subgraphNodeId,
  };
}

/** The centroid of a node set (where the collapsed node is placed). */
function averagePosition(nodes: Node[]): { x: number; y: number } {
  if (nodes.length === 0) {
    return { x: 0, y: 0 };
  }
  const sum = nodes.reduce(
    (acc, n) => ({ x: acc.x + n.position.x, y: acc.y + n.position.y }),
    { x: 0, y: 0 },
  );
  return { x: sum.x / nodes.length, y: sum.y / nodes.length };
}

/**
 * Expand the subgraph node `nodeId` in `graph` back to its interior (fresh ids),
 * reconnecting boundary ports to the exterior endpoints on the parent edges.
 * Returns the new graph, or `null` when `nodeId` is not a subgraph node.
 */
export function expandSubgraph(
  graph: Graph,
  nodeId: string,
  mintId: MintId,
): Graph | null {
  const node = graph.nodes.find((n) => n.id === nodeId);
  if (!node || !isSubgraphNode(node)) {
    return null;
  }
  const expanded = spliceSubgraph(readSubgraph(node), nodeId, graph.edges, mintId);

  // Surviving exterior nodes (the subgraph node removed) + the interior nodes.
  const nextNodes = graph.nodes.filter((n) => n.id !== nodeId);
  nextNodes.push(...expanded.nodes);

  // Parent edges NOT touching the subgraph node carry over; touching ones are
  // replaced by the rewired set; the interior edges are spliced in.
  const passthrough = graph.edges.filter(
    (e) => e.source !== nodeId && e.target !== nodeId,
  );
  const nextEdges = [...passthrough, ...expanded.rewiredParentEdges, ...expanded.edges];

  return { nodes: nextNodes, edges: nextEdges };
}
