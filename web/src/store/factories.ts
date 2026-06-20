// Factories for the editable document entities, typed against the generated
// core-model bindings. Centralizing construction here keeps the on-wire shape
// (and the field defaults) in exactly one place, so the store, the palette, and
// the tests all mint schema-valid Nodes/Edges/Passes/Projects the same way.
import type { Edge } from "../bindings/Edge";
import type { Graph } from "../bindings/Graph";
import type { Node } from "../bindings/Node";
import type { Pass } from "../bindings/Pass";
import type { PassSettings } from "../bindings/PassSettings";
import type { Project } from "../bindings/Project";
import type { Vec2 } from "../bindings/Vec2";
import { PROJECT_SCHEMA_VERSION } from "../model";
import { nextId } from "./ids";

/**
 * The generic placeholder node kind inserted by the palette before the real
 * taxonomy (#49) lands. Kept as a constant so #49 can find every reference.
 */
export const PLACEHOLDER_KIND = "placeholder";

/** Default per-pass render settings — all unset (engine applies §2/§3 defaults). */
export function emptyPassSettings(): PassSettings {
  return {
    scaleX: { scaleType: null, scale: null },
    scaleY: { scaleType: null, scale: null },
    filterLinear: null,
    wrapMode: null,
    mipmapInput: null,
    floatFramebuffer: null,
    srgbFramebuffer: null,
    alias: null,
    frameCountMod: null,
  };
}

/** An empty per-pass node graph. */
export function emptyGraph(): Graph {
  return { nodes: [], edges: [] };
}

/** Create a fresh skeletal Node at a position, with free-form data. */
export function makeNode(
  kind: string,
  position: Vec2,
  data: Record<string, unknown> = {},
): Node {
  return { id: nextId("node"), kind, position, data };
}

/** Create a fresh edge between two node ports. */
export function makeEdge(
  source: string,
  sourcePort: string,
  target: string,
  targetPort: string,
): Edge {
  return { id: nextId("edge"), source, sourcePort, target, targetPort };
}

/** Create a fresh graph-authored pass with an empty graph. */
export function makePass(name: string): Pass {
  return {
    id: nextId("pass"),
    name,
    source: { kind: "graph", graph: emptyGraph() },
    parameters: [],
    settings: emptyPassSettings(),
    references: [],
  };
}

/**
 * A new, single-pass project — the document the editor opens with. Phase 5
 * edits one active graph-pass; #46 adds the multi-pass pipeline view.
 */
export function makeProject(name = "Untitled"): Project {
  return {
    schemaVersion: PROJECT_SCHEMA_VERSION,
    name,
    passes: [makePass("Pass 1")],
    feedbackPass: null,
    pipeline: { aliases: [], availability: [] },
    parameters: [],
    luts: [],
    metadata: { description: null, author: null, createdAt: null, modifiedAt: null },
    libraryRefs: [],
  };
}
