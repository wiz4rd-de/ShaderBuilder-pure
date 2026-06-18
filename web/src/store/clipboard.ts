// Pure copy/paste helpers, factored out of the store so the id-remapping logic
// is unit-testable without a React/zustand harness.
import type { Edge } from "../bindings/Edge";
import type { Node } from "../bindings/Node";
import { makeEdge, makeNode } from "./factories";

/** A detached selection of nodes + the edges fully internal to that selection. */
export interface Clipboard {
  nodes: Node[];
  edges: Edge[];
}

/**
 * Capture the given node ids (and ONLY the edges whose endpoints are BOTH in
 * the set) into a clipboard. Edges that dangle outside the copied set are
 * dropped — paste must never re-point at nodes that weren't copied.
 */
export function captureClipboard(
  nodes: Node[],
  edges: Edge[],
  nodeIds: Iterable<string>,
): Clipboard {
  const ids = new Set(nodeIds);
  const pickedNodes = nodes.filter((n) => ids.has(n.id));
  const pickedEdges = edges.filter((e) => ids.has(e.source) && ids.has(e.target));
  // Deep-clone so later document edits can't mutate the clipboard. `data` can hold
  // nested arrays (e.g. customSnippet ports), so a shallow spread would share them.
  return {
    nodes: pickedNodes.map((n) => ({
      ...n,
      position: { ...n.position },
      data: structuredClone(n.data),
    })),
    edges: pickedEdges.map((e) => ({ ...e })),
  };
}

/**
 * Instantiate clipboard contents as a FRESH set of nodes/edges: every node gets
 * a brand-new id, every internal edge is re-pointed onto the new ids (and gets
 * a fresh id too), and positions are offset so the paste is visibly distinct
 * from its source. Returns the new entities ready to splice into the document.
 */
export function instantiateClipboard(
  clip: Clipboard,
  offset: { x: number; y: number },
): { nodes: Node[]; edges: Edge[] } {
  // Old node id -> freshly-minted node, so edges can be re-pointed.
  const remap = new Map<string, Node>();
  const nodes = clip.nodes.map((src) => {
    const fresh = makeNode(
      src.kind,
      { x: src.position.x + offset.x, y: src.position.y + offset.y },
      // Deep-clone so each paste owns fully independent data (nested arrays like
      // customSnippet ports must not be shared with the clipboard or other pastes).
      structuredClone(src.data),
    );
    remap.set(src.id, fresh);
    return fresh;
  });

  const edges = clip.edges.map((src) => {
    const newSource = remap.get(src.source);
    const newTarget = remap.get(src.target);
    // captureClipboard guarantees both endpoints are present, but guard anyway.
    if (!newSource || !newTarget) {
      return null;
    }
    return makeEdge(newSource.id, src.sourcePort, newTarget.id, src.targetPort);
  });

  return { nodes, edges: edges.filter((e): e is Edge => e !== null) };
}
