// The graph → IR BRIDGE (#49) — the one place that maps the SKELETAL editor
// `Graph` (free-form `kind` + `data`, no port types) onto the typed, FROZEN
// `IrGraph` the Rust `compile_graph` command consumes.
//
// For each skeletal Node it resolves the node-descriptor (by `kind`) and calls
// `descriptor.toNodeOp(data)` to produce the typed `NodeOp`; for each skeletal Edge
// it builds an `IrEdge` of `PortRef → PortRef` using the node ids + the SAME port
// names the checker expects (Sample.coord, Output.color, Expr operands, the
// universal "out"). It also collects the pass-level `Parameter`s and LUT names the
// graph declares, so the caller (#54) passes them to `compile_graph` (the checker
// errors `unknownParam` / unknown-LUT otherwise).
//
// Robustness: a node with an unregistered kind, or whose data fails to lower, is
// DROPPED with a recorded `GraphToIrIssue` (and its incident edges dropped too)
// rather than throwing — a single malformed node should degrade to a clean
// type-error in the rest of the graph, not abort the whole compile. graphToIr is a
// pure function (no Tauri), so it is unit-tested directly.
import type { Edge } from "../bindings/Edge";
import type { Graph } from "../bindings/Graph";
import type { IrEdge } from "../bindings/IrEdge";
import type { IrGraph } from "../bindings/IrGraph";
import type { IrNode } from "../bindings/IrNode";
import type { Parameter } from "../bindings/Parameter";
import { getDescriptor } from "./registry";
import { inlineAllSubgraphs, type MintId } from "./subgraph";
import { NodeLoweringError } from "./types";

/** A non-fatal problem graphToIr hit while lowering one node. */
export interface GraphToIrIssue {
  /** The skeletal node id the problem is about. */
  nodeId: string;
  /** The node's `kind` (for diagnostics). */
  kind: string;
  /** Why the node was dropped. */
  reason: "unknownKind" | "loweringError";
  /** Human-readable detail. */
  message: string;
}

/** The full output of graphToIr — the IrGraph plus the derived compile context. */
export interface GraphToIrResult {
  /** The typed graph to hand to `compile_graph`. */
  ir: IrGraph;
  /**
   * The pass-level `Parameter`s the graph's Param nodes declare. The caller merges
   * these with the pass's authored parameters and passes them to `compile_graph`
   * (and #53 renders sliders for them). De-duplicated by name (first wins).
   */
  parameters: Parameter[];
  /** The LUT names the graph's LUT nodes reference (de-duplicated, declared order). */
  luts: string[];
  /** Non-fatal lowering problems (dropped nodes). Empty on a clean graph. */
  issues: GraphToIrIssue[];
}

/**
 * Lower a skeletal `Graph` to a typed `IrGraph` (+ derived parameters/LUTs/issues).
 * Pure: no IPC, no Tauri runtime — safe to unit-test and to call on every edit.
 */
export function graphToIr(graph: Graph): GraphToIrResult {
  // INLINE collapsed subgraphs FIRST (#57): replace every `kind === "subgraph"`
  // node with its interior nodes/edges (recursively), so the per-node lowering
  // loop — and therefore compile_graph / codegen-slang — only ever sees
  // primitive nodes. No new IR op kind; codegen-slang is untouched. Pure: ids
  // come from a deterministic per-call counter so the result is stable in tests.
  graph = inlineAllSubgraphs(graph, deterministicMintId());

  const nodes: IrNode[] = [];
  const issues: GraphToIrIssue[] = [];
  const parameters: Parameter[] = [];
  const seenParamNames = new Set<string>();
  const luts: string[] = [];
  const seenLuts = new Set<string>();
  // Ids of nodes that successfully lowered — edges touching a dropped node are
  // themselves dropped (a dangling edge would reference a non-existent IrNode).
  const liveIds = new Set<string>();

  for (const node of graph.nodes) {
    const descriptor = getDescriptor(node.kind);
    if (!descriptor) {
      issues.push({
        nodeId: node.id,
        kind: node.kind,
        reason: "unknownKind",
        message: `no descriptor registered for kind "${node.kind}"`,
      });
      continue;
    }
    let op;
    try {
      op = descriptor.toNodeOp(node.data);
    } catch (err) {
      issues.push({
        nodeId: node.id,
        kind: node.kind,
        reason: "loweringError",
        message:
          err instanceof NodeLoweringError || err instanceof Error
            ? err.message
            : String(err),
      });
      continue;
    }

    nodes.push({ id: node.id, op });
    liveIds.add(node.id);

    // Collect a pass Parameter (Param nodes) — de-dupe by pragma name.
    const param = descriptor.toParameter?.(node.data) ?? null;
    if (param && !seenParamNames.has(param.name)) {
      seenParamNames.add(param.name);
      parameters.push(param);
    }
    // Collect a referenced LUT name (LUT nodes) — de-dupe by name.
    const lut = descriptor.toLutName?.(node.data) ?? null;
    if (lut && !seenLuts.has(lut)) {
      seenLuts.add(lut);
      luts.push(lut);
    }
  }

  const edges: IrEdge[] = [];
  for (const edge of graph.edges) {
    // An edge whose endpoint node was dropped (unknown/failed) cannot be lowered.
    if (!liveIds.has(edge.source) || !liveIds.has(edge.target)) {
      continue;
    }
    edges.push(edgeToIrEdge(edge));
  }

  return { ir: { nodes, edges }, parameters, luts, issues };
}

/**
 * A fresh deterministic id minter for the (pure) subgraph-inlining step. Ids are
 * prefixed `inl-<prefix>-<n>` so they never collide with the store's `node-N` /
 * `edge-N` ids, and a new counter per `graphToIr` call keeps results stable +
 * independent across calls (so the equivalence test gets matching topology).
 */
function deterministicMintId(): MintId {
  let n = 0;
  return (prefix: string) => {
    n += 1;
    return `inl-${prefix}-${n}`;
  };
}

/** Map a skeletal Edge to an IrEdge: source-output port → target-input port. */
function edgeToIrEdge(edge: Edge): IrEdge {
  return {
    source: { node: edge.source, port: edge.sourcePort },
    target: { node: edge.target, port: edge.targetPort },
  };
}

/** Convenience: just the IrGraph (drops the derived ctx). */
export function graphToIrGraph(graph: Graph): IrGraph {
  return graphToIr(graph).ir;
}
