// Path-aware graph addressing for subgraph drill-in (#57). The editor edits ONE
// graph at a time; which graph that is depends on the navigation PATH:
//
//   - path == []          → the active pass's top-level `Graph`.
//   - path == [s1]        → the interior body of the `kind=="subgraph"` node
//                           `s1` in the pass graph.
//   - path == [s1, s2]    → the interior of subgraph node `s2` inside `s1`'s
//                           interior, and so on (nested drill-in).
//
// A subgraph node's interior is the `Subgraph` stored in its free-form `data`
// (#56). `resolveGraph` reads the addressed graph; `replaceGraph` writes a new
// project with that graph replaced, rebuilding the `data` chain back up to the
// pass so React/zustand see fresh references the whole way. Both are PURE.
import type { BoundaryPort } from "../bindings/BoundaryPort";
import type { Edge } from "../bindings/Edge";
import type { Graph } from "../bindings/Graph";
import type { Node } from "../bindings/Node";
import type { Project } from "../bindings/Project";
import type { Subgraph } from "../bindings/Subgraph";
import { getDescriptor } from "../nodes/registry";
import { isSubgraphNode, readSubgraph, SUBGRAPH_KIND } from "../nodes/subgraph";

/** An empty graph (avoids importing factories into this pure helper). */
function emptyGraph(): Graph {
  return { nodes: [], edges: [] };
}

/** The active pass's top-level graph, or an empty graph (opaque/code passes). */
function passGraph(project: Project, activePassId: string): Graph {
  const pass = project.passes.find((p) => p.id === activePassId);
  if (pass && pass.source.kind === "graph") {
    return pass.source.graph;
  }
  return emptyGraph();
}

/** A `Subgraph`'s body as a plain `Graph` (its interior nodes + edges). */
function subgraphBody(sub: Subgraph): Graph {
  return { nodes: sub.nodes, edges: sub.edges };
}

/**
 * Resolve the graph addressed by `path` (the chain of subgraph-node ids from the
 * pass graph downward). A path that hits a missing/non-subgraph node resolves to
 * an empty graph (the caller should keep `path` valid).
 */
export function resolveGraph(
  project: Project,
  activePassId: string,
  path: string[],
): Graph {
  let graph = passGraph(project, activePassId);
  for (const nodeId of path) {
    const node = graph.nodes.find((n) => n.id === nodeId);
    if (!node || !isSubgraphNode(node)) {
      return emptyGraph();
    }
    graph = subgraphBody(readSubgraph(node));
  }
  return graph;
}

/**
 * Return a NEW project with the graph addressed by `path` replaced by `next`.
 * The pass (and every subgraph node along `path`) is cloned so references are
 * fresh; the subgraph node's `data` is rewritten to carry the edited interior
 * (its `id`/`name` are preserved; `boundaryPorts` are RECONCILED against the new
 * interior — see `replaceInGraph`). Untouched siblings are shared.
 */
export function replaceGraph(
  project: Project,
  activePassId: string,
  path: string[],
  next: Graph,
): Project {
  const nextPassGraph =
    path.length === 0
      ? next
      : replaceInGraph(passGraph(project, activePassId), path, next);
  return {
    ...project,
    passes: project.passes.map((p) => {
      if (p.id !== activePassId || p.source.kind !== "graph") {
        return p;
      }
      return { ...p, source: { ...p.source, graph: nextPassGraph } };
    }),
  };
}

/**
 * Whether `(interiorNode, interiorPort)` is still a live endpoint in `interior`:
 * the node must exist AND expose `interiorPort` on the side the boundary uses (an
 * `in` boundary feeds an interior INPUT, an `out` boundary carries an interior
 * OUTPUT). A deleted node, or a port the node no longer declares, fails.
 */
function boundaryEndpointLives(bp: BoundaryPort, interior: Graph): boolean {
  const node = interior.nodes.find((n) => n.id === bp.interiorNode);
  if (!node) {
    return false;
  }
  const descriptor = getDescriptor(node.kind);
  if (!descriptor) {
    return false;
  }
  const ports =
    bp.direction === "in" ? descriptor.inputs(node.data) : descriptor.outputs(node.data);
  return ports.some((p) => p.name === bp.interiorPort);
}

/**
 * Reconcile a subgraph's `boundaryPorts` against an edited `interior` (#57 fix):
 * keep only the ports whose interior endpoint still exists, and report the names
 * of the ports pruned so the caller can drop the now-dangling EXTERIOR edges in
 * the SAME mutation. Without this, an exterior edge stays attached to a port the
 * inlining step can't resolve and `graphToIr` SILENTLY DROPS it at compile time.
 */
function reconcileBoundaryPorts(
  boundaryPorts: BoundaryPort[],
  interior: Graph,
): { kept: BoundaryPort[]; prunedNames: Set<string> } {
  const kept: BoundaryPort[] = [];
  const prunedNames = new Set<string>();
  for (const bp of boundaryPorts) {
    if (boundaryEndpointLives(bp, interior)) {
      kept.push(bp);
    } else {
      prunedNames.add(bp.name);
    }
  }
  return { kept, prunedNames };
}

/**
 * Drop the exterior edges in `graph` that connect to `subgraphNodeId` on a port
 * name in `prunedNames` (an `in` boundary is an edge whose TARGET is that port;
 * an `out` boundary an edge whose SOURCE is). Returns `graph` unchanged when no
 * edge is affected (so untouched levels keep their reference identity).
 */
function pruneExteriorEdges(
  graph: Graph,
  subgraphNodeId: string,
  prunedNames: Set<string>,
): Graph {
  if (prunedNames.size === 0) {
    return graph;
  }
  const edges = graph.edges.filter((e) => {
    const intoPruned =
      e.target === subgraphNodeId && prunedNames.has(e.targetPort);
    const outOfPruned =
      e.source === subgraphNodeId && prunedNames.has(e.sourcePort);
    return !intoPruned && !outOfPruned;
  });
  return edges.length === graph.edges.length ? graph : { ...graph, edges };
}

/** Recursively rebuild `graph` with the subgraph addressed by `path` set to `next`. */
function replaceInGraph(graph: Graph, path: string[], next: Graph): Graph {
  const [head, ...rest] = path;
  // Names of boundary ports pruned on `head` so we can drop the matching exterior
  // edges (the parent edges live in THIS graph) in the same mutation.
  let prunedNames = new Set<string>();
  const nodes = graph.nodes.map((node) => {
    if (node.id !== head || !isSubgraphNode(node)) {
      return node;
    }
    const sub = readSubgraph(node);
    const newBody =
      rest.length === 0 ? next : replaceInGraph(subgraphBody(sub), rest, next);
    // Reconcile boundary ports against the edited interior: prune any port whose
    // interior endpoint no longer exists so no boundary dangles. (When editing a
    // DEEPER interior, `replaceInGraph` already pruned that level's exterior edges
    // — which live in `sub`'s body — so `newBody` is internally consistent here.)
    const { kept, prunedNames: pruned } = reconcileBoundaryPorts(
      sub.boundaryPorts,
      newBody,
    );
    prunedNames = pruned;
    const nextSub: Subgraph = {
      ...sub,
      nodes: newBody.nodes,
      edges: newBody.edges,
      boundaryPorts: kept,
    };
    return { ...node, kind: SUBGRAPH_KIND, data: nextSub as unknown as Record<string, unknown> };
  });
  return pruneExteriorEdges({ ...graph, nodes }, head, prunedNames);
}

/** Read a subgraph node's typed body from a graph, or null if absent/not one. */
export function subgraphAt(graph: Graph, nodeId: string): Subgraph | null {
  const node = graph.nodes.find((n) => n.id === nodeId);
  return node && isSubgraphNode(node) ? readSubgraph(node) : null;
}

/** Re-export the shared splice types for the store's expand action. */
export type { Node, Edge };
