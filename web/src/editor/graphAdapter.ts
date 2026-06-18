// Adapter between the core-model skeletal `Graph` (the document) and the React
// Flow node/edge shapes (the view). The store is authoritative; these functions
// DERIVE the RF arrays that <ReactFlow> renders, and translate selection back.
//
// Mapping (Architecture §A): RF node.type ⇄ core Node.kind, RF node.data ⇄ core
// Node.data, RF node.position ⇄ core Vec2. RF edge.sourceHandle/targetHandle ⇄
// core Edge.sourcePort/targetPort.
import type { Edge as RfEdge, Node as RfNode } from "@xyflow/react";

import type { Edge } from "../bindings/Edge";
import type { Graph } from "../bindings/Graph";
import type { Node } from "../bindings/Node";
import type { Selection } from "../store/documentStore";

/** The RF node data we carry: the core node's free-form data plus its label. */
export type EditorNodeData = Record<string, unknown> & { label?: string };

export type EditorRfNode = RfNode<EditorNodeData>;
export type EditorRfEdge = RfEdge;

/** Project a document Node onto a React Flow node (controlled-mode shape). */
export function toRfNode(node: Node, selected: boolean): EditorRfNode {
  return {
    id: node.id,
    // RF node.type ⇄ core Node.kind. The default RF renderer is used until the
    // taxonomy (#49) registers per-kind node components.
    type: "default",
    position: { x: node.position.x, y: node.position.y },
    data: { ...node.data, label: deriveLabel(node) },
    selected,
  };
}

/** Project a document Edge onto a React Flow edge. */
export function toRfEdge(edge: Edge, selected: boolean): EditorRfEdge {
  return {
    id: edge.id,
    source: edge.source,
    target: edge.target,
    sourceHandle: edge.sourcePort || null,
    targetHandle: edge.targetPort || null,
    selected,
  };
}

/** Build the full RF node/edge arrays for a graph + current selection. */
export function toRfGraph(
  graph: Graph,
  selection: Selection,
): { nodes: EditorRfNode[]; edges: EditorRfEdge[] } {
  const selNodes = new Set(selection.nodeIds);
  const selEdges = new Set(selection.edgeIds);
  return {
    nodes: graph.nodes.map((n) => toRfNode(n, selNodes.has(n.id))),
    edges: graph.edges.map((e) => toRfEdge(e, selEdges.has(e.id))),
  };
}

/** A human label for a node — its data.label, else its kind. */
function deriveLabel(node: Node): string {
  const fromData = node.data["label"];
  if (typeof fromData === "string" && fromData.length > 0) {
    return fromData;
  }
  return node.kind;
}
