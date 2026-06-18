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
import type { Edge } from "../bindings/Edge";
import type { Graph } from "../bindings/Graph";
import type { Node } from "../bindings/Node";
import type { Project } from "../bindings/Project";
import type { Subgraph } from "../bindings/Subgraph";
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
 * (its `boundaryPorts`/`id`/`name` are preserved). Untouched siblings are shared.
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

/** Recursively rebuild `graph` with the subgraph addressed by `path` set to `next`. */
function replaceInGraph(graph: Graph, path: string[], next: Graph): Graph {
  const [head, ...rest] = path;
  return {
    ...graph,
    nodes: graph.nodes.map((node) => {
      if (node.id !== head || !isSubgraphNode(node)) {
        return node;
      }
      const sub = readSubgraph(node);
      const newBody =
        rest.length === 0 ? next : replaceInGraph(subgraphBody(sub), rest, next);
      const nextSub: Subgraph = {
        ...sub,
        nodes: newBody.nodes,
        edges: newBody.edges,
      };
      return { ...node, kind: SUBGRAPH_KIND, data: nextSub as unknown as Record<string, unknown> };
    }),
  };
}

/** Read a subgraph node's typed body from a graph, or null if absent/not one. */
export function subgraphAt(graph: Graph, nodeId: string): Subgraph | null {
  const node = graph.nodes.find((n) => n.id === nodeId);
  return node && isSubgraphNode(node) ? readSubgraph(node) : null;
}

/** Re-export the shared splice types for the store's expand action. */
export type { Node, Edge };
