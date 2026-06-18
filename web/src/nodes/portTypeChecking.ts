// In-editor CONNECTION TYPE-CHECKING (#65) — the drag-time edge-legality guard.
//
// The AUTHORITATIVE type rules live FROZEN in Rust (`core_model::ir`): `PortType`
// + `assignable_to` (= broadcast UNION implicit-widen), the `swizzle_result`
// typing, and the `Sample.coord` tightening (a UV coord must be an EXACT vec2 — no
// `Float→vec2` broadcast). This module is a faithful TypeScript PORT of that
// predicate so the editor can refuse an illegal wire at DROP time with no IPC
// latency. Its agreement with the Rust checker is not assumed — it is PROVEN by a
// cross-language parity golden (`__goldens__/connectionLegality.json`, generated
// by `crates/ir/tests/connection_parity.rs`) that `portTypeChecking.test.ts`
// asserts this predicate reproduces row-for-row. Drift between the two fails CI.
//
// What this guard does and does NOT enforce:
//  * It enforces the structural EDGE-ASSIGNMENT rule (source output type vs sink
//    input port type) — exactly what `compile_graph` flags as `typeMismatch`.
//  * It does NOT enforce Expr/Math operand constraints (Int+vec mixing, arity,
//    swizzle-mask legality) — those need full operand inference and are DEFERRED
//    to `compile_graph` diagnostics, mirroring the checker's polymorphic treatment
//    of Expr operands (the structural assignment there is reflexive).
import type { Edge } from "../bindings/Edge";
import type { Graph } from "../bindings/Graph";
import type { Node } from "../bindings/Node";
import type { PortType } from "../bindings/PortType";
import { getDescriptor } from "./registry";

// ---------------------------------------------------------------------------
// The frozen PortType predicates (a 1:1 port of core_model::ir::PortType).
// ---------------------------------------------------------------------------

/** The float-vector types (`vec2`/`vec3`/`vec4`). */
function isVector(t: PortType): boolean {
  return t === "vec2" || t === "vec3" || t === "vec4";
}

/**
 * Whether a value of `src` may be **scalar-broadcast** to `tgt` — a `float`
 * broadcasts to any float vector (GLSL `vecN(x)`); a type always broadcasts to
 * itself. Mirrors `PortType::broadcast_to`.
 */
function broadcastTo(src: PortType, tgt: PortType): boolean {
  if (src === tgt) {
    return true;
  }
  return src === "float" && isVector(tgt);
}

/**
 * Whether a value of `src` **implicitly widens** to `tgt` — the only widening is
 * the scalar promotion `int → float`; a type trivially widens to itself. Mirrors
 * `PortType::implicit_widen_to`.
 */
function implicitWidenTo(src: PortType, tgt: PortType): boolean {
  if (src === tgt) {
    return true;
  }
  return src === "int" && tgt === "float";
}

/**
 * Whether `src` is **assignable** to `tgt`, accepting exact match, the `int →
 * float` widen, and the `float → vecN` broadcast. Mirrors
 * `PortType::assignable_to`.
 */
export function assignableTo(src: PortType, tgt: PortType): boolean {
  return implicitWidenTo(src, tgt) || broadcastTo(src, tgt);
}

/** The accessor sets a swizzle mask char may come from, in component order. */
const SWIZZLE_SETS = ["xyzw", "rgba", "stpq"] as const;

/** The float-vector type with `n` components (`1 → float`, …`4 → vec4`), or null. */
function floatWithComponents(n: number): PortType | null {
  switch (n) {
    case 1:
      return "float";
    case 2:
      return "vec2";
    case 3:
      return "vec3";
    case 4:
      return "vec4";
    default:
      return null;
  }
}

/** The component count of a scalar/vector type (`null` for `sampler2D`). */
function componentCount(t: PortType): number | null {
  switch (t) {
    case "float":
    case "int":
    case "bool":
      return 1;
    case "vec2":
      return 2;
    case "vec3":
      return 3;
    case "vec4":
      return 4;
    case "sampler2D":
      return null;
  }
}

/**
 * The [`PortType`] resulting from applying a **swizzle** `mask` to `base`, or
 * `null` if the swizzle is illegal. A faithful port of `PortType::swizzle_result`:
 * the base must be a float scalar/vector, the mask length 1..4 from a single
 * accessor set, every selected component in range.
 */
export function swizzleResult(base: PortType, mask: string): PortType | null {
  // Sampler / int / bool cannot be swizzled.
  if (!(base === "float" || isVector(base))) {
    return null;
  }
  const baseComponents = componentCount(base);
  if (baseComponents === null) {
    return null;
  }
  const len = [...mask].length;
  if (len === 0 || len > 4) {
    return null;
  }
  let chosenSet: number | null = null;
  for (const ch of mask) {
    let found: { setIdx: number; compIdx: number } | null = null;
    for (let setIdx = 0; setIdx < SWIZZLE_SETS.length; setIdx += 1) {
      const compIdx = SWIZZLE_SETS[setIdx]!.indexOf(ch);
      if (compIdx >= 0) {
        found = { setIdx, compIdx };
        break;
      }
    }
    if (!found) {
      return null;
    }
    if (chosenSet === null) {
      chosenSet = found.setIdx;
    } else if (chosenSet !== found.setIdx) {
      return null; // mixed accessor sets
    }
    if (found.compIdx >= baseComponents) {
      return null; // selecting a component the base doesn't have
    }
  }
  return floatWithComponents(len);
}

// ---------------------------------------------------------------------------
// The connection-legality predicate (a port of core_model::ir::connection_legal).
// ---------------------------------------------------------------------------

/**
 * The kind of sink an edge is dropped onto — the editor-side mirror of Rust's
 * `ConnectionTarget`. Resolved from a drop target's descriptor + targeted port.
 */
export type ConnectionTarget =
  | { kind: "assignable"; type: PortType }
  | { kind: "sampleCoord" }
  | { kind: "exprOperand" };

/**
 * Whether a value of type `srcType` may legally feed a sink described by
 * `target` — a faithful port of `core_model::ir::connection_legal`.
 *
 *  * `assignable(ty)`  → the documented {@link assignableTo} rule.
 *  * `sampleCoord`     → exact match only (no scalar broadcast into a UV).
 *  * `exprOperand`     → always legal structurally (operand-type checks are
 *                        deferred to `compile_graph`).
 */
export function connectionLegal(srcType: PortType, target: ConnectionTarget): boolean {
  switch (target.kind) {
    case "assignable":
      return assignableTo(srcType, target.type);
    case "sampleCoord":
      return srcType === "vec2";
    case "exprOperand":
      return true;
  }
}

// ---------------------------------------------------------------------------
// Resolving real graph ports into types + ConnectionTargets (descriptor-driven).
// ---------------------------------------------------------------------------

/**
 * The OUTPUT type a source node's `port` produces, accounting for data-derived
 * port types (a Swizzle's output depends on its stored mask, a Combine's on its
 * target type, a CustomSnippet's on its editable ports). Reads the node's CURRENT
 * `data` via its descriptor — never a cached spec. Returns `null` when the node
 * kind / port is unresolvable (treated as "cannot judge" by callers).
 */
export function sourceOutputType(node: Node, port: string): PortType | null {
  const descriptor = getDescriptor(node.kind);
  if (!descriptor) {
    return null;
  }
  const spec = descriptor.outputs(node.data).find((p) => p.name === port);
  return spec ? spec.type : null;
}

/**
 * The declared INPUT type of a target node's `port` (data-derived for editable
 * ports). Returns `null` when unresolvable.
 */
export function targetInputType(node: Node, port: string): PortType | null {
  const descriptor = getDescriptor(node.kind);
  if (!descriptor) {
    return null;
  }
  const spec = descriptor.inputs(node.data).find((p) => p.name === port);
  return spec ? spec.type : null;
}

/**
 * Classify a target node's input `port` into the {@link ConnectionTarget} case the
 * legality predicate turns on — the editor-side equivalent of the checker's
 * `input_port_type` + `edge_assignable` dispatch.
 *
 * The classification keys off the node's LOWERED `NodeOp` kind (the same bridge
 * `graphToIr` uses), so it matches the IR exactly:
 *   * an `expr` op operand            → polymorphic (`exprOperand`)
 *   * a `sample` op `coord` port      → the tightened vec2 (`sampleCoord`)
 *   * any other typed sink            → `assignable(<declared type>)`
 *
 * Subgraph nodes (whose `toNodeOp` deliberately throws — they are inlined before
 * lowering) and any node whose data fails to lower fall back to the descriptor's
 * declared input type as an `assignable` sink; the post-inline `compile_graph`
 * remains the authority for the interior wiring.
 */
export function classifyTarget(node: Node, port: string): ConnectionTarget | null {
  const declared = targetInputType(node, port);
  if (declared === null) {
    return null; // no such input port on this node
  }
  const descriptor = getDescriptor(node.kind);
  let op;
  try {
    op = descriptor?.toNodeOp(node.data);
  } catch {
    op = undefined; // e.g. subgraph guard / malformed data — use the declared type
  }
  if (op) {
    if (op.kind === "expr") {
      return { kind: "exprOperand" };
    }
    if (op.kind === "sample" && port === "coord") {
      return { kind: "sampleCoord" };
    }
  }
  return { kind: "assignable", type: declared };
}

/** The outcome of judging a candidate connection between two graph ports. */
export interface ConnectionVerdict {
  /** Whether the edge is structurally legal (may be created). */
  legal: boolean;
  /** The resolved source-output type, if known. */
  sourceType: PortType | null;
  /** How the legal edge coerces the source value into the sink — for marking. */
  coercion: ConnectionCoercion;
}

/**
 * How a (legal) connection adapts the source value to the sink. Drives the
 * visual marking on a created edge:
 *  * `exact`     — types match identically.
 *  * `widen`     — an `int → float` implicit promotion.
 *  * `broadcast` — a `float → vecN` scalar broadcast.
 *  * `none`      — illegal, or a polymorphic/unknown sink (no marking).
 */
export type ConnectionCoercion = "exact" | "widen" | "broadcast" | "none";

/** Classify how a legal `assignable` edge coerces `src` into `tgt`. */
function coercionFor(src: PortType, tgt: PortType): ConnectionCoercion {
  if (src === tgt) {
    return "exact";
  }
  if (src === "int" && tgt === "float") {
    return "widen";
  }
  if (src === "float" && isVector(tgt)) {
    return "broadcast";
  }
  return "none";
}

/**
 * Judge a candidate connection `source.sourcePort → target.targetPort` within
 * `graph`. The single entry point the canvas `isValidConnection` hook + the
 * stale-edge re-validator consult.
 *
 * Resolves the source-output type and the target sink classification from the
 * live node `data`, then applies {@link connectionLegal}. A port that cannot be
 * resolved (unknown kind / dropped port) is treated as LEGAL (`legal: true`,
 * `coercion: "none"`) so the editor never blocks an edge it cannot judge — the
 * authoritative `compile_graph` still runs.
 */
export function judgeConnection(
  graph: Graph,
  source: string,
  sourcePort: string,
  target: string,
  targetPort: string,
): ConnectionVerdict {
  const srcNode = graph.nodes.find((n) => n.id === source);
  const tgtNode = graph.nodes.find((n) => n.id === target);
  if (!srcNode || !tgtNode) {
    return { legal: false, sourceType: null, coercion: "none" };
  }
  const srcType = sourceOutputType(srcNode, sourcePort);
  const classified = classifyTarget(tgtNode, targetPort);
  // If either endpoint is unresolvable, don't block — defer to compile_graph.
  if (srcType === null || classified === null) {
    return { legal: true, sourceType: srcType, coercion: "none" };
  }
  const legal = connectionLegal(srcType, classified);
  const coercion =
    legal && classified.kind === "assignable"
      ? coercionFor(srcType, classified.type)
      : "none";
  return { legal, sourceType: srcType, coercion };
}

/**
 * Whether an ALREADY-PRESENT edge is now ILLEGAL given the current graph (a node's
 * kind/data changed since the edge was drawn — e.g. a Combine retargeted to vec3
 * now feeds a vec4 sink, or a Swizzle mask shortened). Used to flag stale edges
 * inline. Only returns true for a CONFIDENT rejection (both endpoints resolved);
 * an unjudgeable edge is never flagged here (compile_graph owns that).
 */
export function edgeIsIllegal(graph: Graph, edge: Edge): boolean {
  const srcNode = graph.nodes.find((n) => n.id === edge.source);
  const tgtNode = graph.nodes.find((n) => n.id === edge.target);
  if (!srcNode || !tgtNode) {
    return false;
  }
  const srcType = sourceOutputType(srcNode, edge.sourcePort);
  const classified = classifyTarget(tgtNode, edge.targetPort);
  if (srcType === null || classified === null) {
    return false; // cannot judge — defer to compile_graph
  }
  return !connectionLegal(srcType, classified);
}
