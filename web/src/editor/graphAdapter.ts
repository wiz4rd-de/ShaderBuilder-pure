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
import { getDescriptor } from "../nodes/registry";
import { judgeConnection } from "../nodes/portTypeChecking";
import type { Selection } from "../store/documentStore";

/** The RF node data we carry: the core node's free-form data plus its label. */
export type EditorNodeData = Record<string, unknown> & { label?: string };

export type EditorRfNode = RfNode<EditorNodeData>;
export type EditorRfEdge = RfEdge;

/** Project a document Node onto a React Flow node (controlled-mode shape). */
export function toRfNode(node: Node, selected: boolean): EditorRfNode {
  return {
    id: node.id,
    // RF node.type ⇄ core Node.kind. The taxonomy registry (#49) registers a
    // component for every kind (see nodes/nodeTypes.ts); unknown kinds fall back
    // to TaxonomyNode's "unknown node" card.
    type: node.kind,
    position: { x: node.position.x, y: node.position.y },
    data: { ...node.data, label: deriveLabel(node) },
    selected,
  };
}

/**
 * Project a document Edge onto a React Flow edge. `graph` lets the adapter
 * re-judge the edge's type-legality + coercion against the LIVE node data (#65):
 *  * an edge made ILLEGAL by a later node-type/data change is tagged
 *    `editor-edge--invalid` (a red, dashed wire flagging it inline — the
 *    authoritative compile diagnostic still reports it on the sink node);
 *  * a still-legal but COERCED edge (an `int → float` widen or a `float → vecN`
 *    broadcast) is tagged so the implicit conversion is visible on the wire.
 */
export function toRfEdge(edge: Edge, selected: boolean, graph: Graph): EditorRfEdge {
  const verdict = judgeConnection(
    graph,
    edge.source,
    edge.sourcePort,
    edge.target,
    edge.targetPort,
  );
  const classNames = ["editor-edge"];
  if (!verdict.legal) {
    classNames.push("editor-edge--invalid");
  } else if (verdict.coercion === "widen" || verdict.coercion === "broadcast") {
    classNames.push(`editor-edge--${verdict.coercion}`);
  }
  return {
    id: edge.id,
    source: edge.source,
    target: edge.target,
    sourceHandle: edge.sourcePort || null,
    targetHandle: edge.targetPort || null,
    selected,
    className: classNames.join(" "),
    // A coerced wire carries a small label noting the implicit conversion; an
    // illegal one is annotated so hovering/reading the wire explains why.
    label: !verdict.legal
      ? "type mismatch"
      : verdict.coercion === "widen"
        ? "int→float"
        : verdict.coercion === "broadcast"
          ? `→${edgeBroadcastTarget(graph, edge)}`
          : undefined,
  };
}

/** The vecN a broadcast edge widens into (for the wire label), or "vec". */
function edgeBroadcastTarget(graph: Graph, edge: Edge): string {
  const tgt = graph.nodes.find((n) => n.id === edge.target);
  if (!tgt) {
    return "vec";
  }
  const spec = getDescriptor(tgt.kind)
    ?.inputs(tgt.data)
    .find((p) => p.name === edge.targetPort);
  return spec?.type ?? "vec";
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
    edges: graph.edges.map((e) => toRfEdge(e, selEdges.has(e.id), graph)),
  };
}

/**
 * A human label for a node: a user-set `data.label` wins, then the descriptor's
 * data-derived `title` (e.g. a subgraph's `name`), else its kind.
 */
function deriveLabel(node: Node): string {
  const fromData = node.data["label"];
  if (typeof fromData === "string" && fromData.length > 0) {
    return fromData;
  }
  const derived = getDescriptor(node.kind)?.title?.(node.data);
  if (typeof derived === "string" && derived.length > 0) {
    return derived;
  }
  return node.kind;
}
