// Subgraph splice/inline core (#57) — the PURE id-remapping logic shared by the
// `graphToIr` inlining step (lowering) and the document store's expand action
// (live editing). A collapsed subgraph is an ordinary skeletal `Node` whose
// `kind === "subgraph"` and whose free-form `data` IS a serialized `Subgraph`
// (#56). "Expanding" one such node means: clone its interior nodes/edges with
// FRESH ids, splice them into the parent graph, and rewire every parent edge
// that touched the collapsed node's boundary ports onto the corresponding
// interior endpoint — so the resulting graph is semantically identical to the
// un-collapsed one. graphToIr calls this recursively until no subgraph nodes
// remain, which is why codegen-slang never sees a "subgraph" op.
import type { BoundaryPort } from "../bindings/BoundaryPort";
import type { Edge } from "../bindings/Edge";
import type { Graph } from "../bindings/Graph";
import type { Node } from "../bindings/Node";
import type { Subgraph } from "../bindings/Subgraph";

/** The skeletal node `kind` a collapsed subgraph is stored under. */
export const SUBGRAPH_KIND = "subgraph";

/** Mint a fresh unique id for the given prefix (`node`/`edge`). */
export type MintId = (prefix: string) => string;

/** Whether a skeletal node is a collapsed subgraph. */
export function isSubgraphNode(node: Node): boolean {
  return node.kind === SUBGRAPH_KIND;
}

/**
 * Read a node's `data` as a `Subgraph`, filling in any missing field with an
 * empty default so a malformed/partial `data` degrades to an empty interior
 * rather than throwing. The id/name fall back to the node id / a placeholder.
 */
export function readSubgraph(node: Node): Subgraph {
  const data = node.data as Partial<Subgraph> | undefined;
  return {
    id: typeof data?.id === "string" ? data.id : node.id,
    name: typeof data?.name === "string" ? data.name : "Subgraph",
    nodes: Array.isArray(data?.nodes) ? data.nodes : [],
    edges: Array.isArray(data?.edges) ? data.edges : [],
    boundaryPorts: Array.isArray(data?.boundaryPorts) ? data.boundaryPorts : [],
  };
}

/** The interior + rewired-parent fragments produced by expanding one subgraph. */
export interface ExpandedSubgraph {
  /** The interior nodes, re-id'd (deep-cloned `data`). */
  nodes: Node[];
  /** The interior edges, re-id'd and re-pointed onto the new interior nodes. */
  edges: Edge[];
  /**
   * The parent edges that touched the collapsed node, rewired onto the matching
   * interior endpoint (fresh interior-node id + the boundary's `interiorPort`).
   * A parent edge whose boundary port has no match is DROPPED (returned absent).
   */
  rewiredParentEdges: Edge[];
}

/**
 * Expand ONE collapsed subgraph node: clone its interior with fresh ids and
 * rewire the parent edges incident on `subgraphNodeId`'s boundary ports.
 *
 * - For each INPUT boundary port: a parent edge whose TARGET is
 *   `(subgraphNodeId, port.name)` is redirected to target the interior endpoint
 *   `(newInteriorNode, port.interiorPort)` — the value still flows inward.
 * - For each OUTPUT boundary port: a parent edge whose SOURCE is
 *   `(subgraphNodeId, port.name)` is redirected to source from
 *   `(newInteriorNode, port.interiorPort)` — the interior value still flows out.
 *
 * `mintId` makes id generation injectable so the pure `graphToIr` path can use a
 * deterministic counter (stable tests) while the store uses the global `nextId`.
 */
export function expandSubgraphNode(
  subgraph: Subgraph,
  subgraphNodeId: string,
  parentEdges: Edge[],
  mintId: MintId,
): ExpandedSubgraph {
  // Old interior node id -> fresh id. Built before edges so both ends remap.
  const idMap = new Map<string, string>();
  const nodes: Node[] = subgraph.nodes.map((src) => {
    const freshId = mintId("node");
    idMap.set(src.id, freshId);
    return {
      ...src,
      id: freshId,
      position: { ...src.position },
      data: structuredClone(src.data),
    };
  });

  const edges: Edge[] = subgraph.edges.map((src) => ({
    id: mintId("edge"),
    source: idMap.get(src.source) ?? src.source,
    sourcePort: src.sourcePort,
    target: idMap.get(src.target) ?? src.target,
    targetPort: src.targetPort,
  }));

  // Index the boundary ports by (name, direction) so a parent edge endpoint on
  // the collapsed node resolves to its interior endpoint in O(1).
  const inByName = new Map<string, BoundaryPort>();
  const outByName = new Map<string, BoundaryPort>();
  for (const bp of subgraph.boundaryPorts) {
    (bp.direction === "in" ? inByName : outByName).set(bp.name, bp);
  }

  const rewiredParentEdges: Edge[] = [];
  for (const edge of parentEdges) {
    const intoCollapsed = edge.target === subgraphNodeId;
    const outOfCollapsed = edge.source === subgraphNodeId;
    if (!intoCollapsed && !outOfCollapsed) {
      continue; // not incident on the collapsed node — handled by the caller
    }
    const next: Edge = { ...edge };
    if (intoCollapsed) {
      const bp = inByName.get(edge.targetPort);
      if (!bp) {
        continue; // dangling — boundary port gone; drop the edge
      }
      next.target = idMap.get(bp.interiorNode) ?? bp.interiorNode;
      next.targetPort = bp.interiorPort;
    }
    if (outOfCollapsed) {
      const bp = outByName.get(edge.sourcePort);
      if (!bp) {
        continue;
      }
      next.source = idMap.get(bp.interiorNode) ?? bp.interiorNode;
      next.sourcePort = bp.interiorPort;
    }
    rewiredParentEdges.push(next);
  }

  return { nodes, edges, rewiredParentEdges };
}

/**
 * Recursively inline EVERY collapsed subgraph node in `graph` (and any subgraphs
 * nested inside them) so the result contains only primitive nodes. Pure: id
 * generation is injected via `mintId`. The relative order of non-subgraph nodes
 * is preserved; an expanded subgraph's interior is spliced in where the
 * collapsed node sat.
 */
export function inlineAllSubgraphs(graph: Graph, mintId: MintId): Graph {
  // A single pass that expands every top-level subgraph node; recurse on the
  // result until no subgraph nodes remain (handles nesting + interiors that
  // themselves contained subgraph nodes).
  let current = graph;
  // Guard against a pathological cycle in malformed data (shouldn't happen —
  // subgraph bodies are trees by construction) so we never loop forever.
  for (let depth = 0; depth < 1000; depth += 1) {
    if (!current.nodes.some(isSubgraphNode)) {
      return current;
    }
    current = inlineOnePass(current, mintId);
  }
  return current;
}

/** A resolved boundary endpoint: an interior (re-id'd node, interior port). */
interface BoundaryTarget {
  node: string;
  port: string;
}

/**
 * One inlining pass: expand EVERY top-level subgraph node in `graph` at once.
 *
 * Done in one pass (rather than node-by-node) so an edge that connects TWO
 * collapsed subgraph nodes — `(subA.out) → (subB.in)` — is rewired exactly once
 * with BOTH endpoints resolved to their respective interiors, instead of being
 * emitted twice.
 */
function inlineOnePass(graph: Graph, mintId: MintId): Graph {
  const nodes: Node[] = [];
  const interiorEdges: Edge[] = [];
  const subgraphNodeIds = new Set(
    graph.nodes.filter(isSubgraphNode).map((n) => n.id),
  );
  // (collapsedNodeId, boundary-port-name) -> resolved interior endpoint, split
  // by direction so a parent edge end resolves against the right side.
  const inResolved = new Map<string, BoundaryTarget>();
  const outResolved = new Map<string, BoundaryTarget>();
  const key = (nodeId: string, port: string) => `${nodeId}\u0000${port}`;

  for (const node of graph.nodes) {
    if (!isSubgraphNode(node)) {
      nodes.push(node);
      continue;
    }
    const subgraph = readSubgraph(node);
    const idMap = new Map<string, string>();
    for (const src of subgraph.nodes) {
      const freshId = mintId("node");
      idMap.set(src.id, freshId);
      nodes.push({
        ...src,
        id: freshId,
        position: { ...src.position },
        data: structuredClone(src.data),
      });
    }
    for (const src of subgraph.edges) {
      interiorEdges.push({
        id: mintId("edge"),
        source: idMap.get(src.source) ?? src.source,
        sourcePort: src.sourcePort,
        target: idMap.get(src.target) ?? src.target,
        targetPort: src.targetPort,
      });
    }
    for (const bp of subgraph.boundaryPorts) {
      const target: BoundaryTarget = {
        node: idMap.get(bp.interiorNode) ?? bp.interiorNode,
        port: bp.interiorPort,
      };
      (bp.direction === "in" ? inResolved : outResolved).set(
        key(node.id, bp.name),
        target,
      );
    }
  }

  // Rewire each parent edge: resolve any endpoint that lands on a collapsed
  // node through its boundary mapping; drop the edge if a boundary is missing.
  const rewired: Edge[] = [];
  for (const edge of graph.edges) {
    const next: Edge = { ...edge };
    const srcResolved = outResolved.get(key(edge.source, edge.sourcePort));
    const tgtResolved = inResolved.get(key(edge.target, edge.targetPort));
    if (subgraphNodeIds.has(edge.source)) {
      if (!srcResolved) {
        continue; // dangling boundary — drop
      }
      next.source = srcResolved.node;
      next.sourcePort = srcResolved.port;
    }
    if (subgraphNodeIds.has(edge.target)) {
      if (!tgtResolved) {
        continue;
      }
      next.target = tgtResolved.node;
      next.targetPort = tgtResolved.port;
    }
    rewired.push(next);
  }

  return { nodes, edges: [...rewired, ...interiorEdges] };
}
