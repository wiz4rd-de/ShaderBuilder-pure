// The editor document store (#45) — the single source of truth for the editable
// document. React Flow nodes/edges are DERIVED from this store (see
// editor/graphAdapter.ts); the canvas never holds authoritative state.
//
// Design notes
// ------------
// * The document is the core-model `Project`. Phase 5 edits ONE active pass's
//   skeletal `Graph` at a time (`activePassId`); the store already carries the
//   full passes collection so #46 can add the multi-pass pipeline view without
//   reshaping the store.
// * Undo/redo is snapshot-based (see snapshot.ts): mutating actions take a
//   whole-document snapshot BEFORE applying, so undo restores the exact prior
//   state. Discrete edits (addNode/addEdge/delete/paste) push one entry each.
// * Drag moves are COALESCED into one entry: `beginInteraction()` stashes a
//   pre-edit baseline on drag-start, `applyNodeChanges` mutates positions live
//   WITHOUT touching history, and `commit()` on drag-stop pushes that single
//   baseline.
// * The action surface is deliberately tight and named for downstream issues:
//   addNode, addEdge, moveNodes, removeSelection, copy, paste, duplicate, undo,
//   redo, toSnapshot/fromSnapshot. Everything else derives from these.
import {
  applyEdgeChanges,
  applyNodeChanges,
  type EdgeChange,
  type NodeChange,
} from "@xyflow/react";
import { create } from "zustand";

import type { Diagnostic } from "../bindings/Diagnostic";
import type { Graph } from "../bindings/Graph";
import type { LibraryPayload } from "../bindings/LibraryPayload";
import type { Node } from "../bindings/Node";
import type { PassSettings } from "../bindings/PassSettings";
import type { Project } from "../bindings/Project";
import type { Vec2 } from "../bindings/Vec2";
import {
  addPass,
  passToGraph,
  passToWholePassCode,
  removePass,
  reorderPass,
  setWholePassSource,
} from "../pipeline/passOps";
import {
  captureClipboard,
  instantiateClipboard,
  type Clipboard,
} from "./clipboard";
import {
  emptyGraph,
  makeEdge,
  makeNode,
  makePass,
  makeProject,
  PLACEHOLDER_KIND,
} from "./factories";
import { collapseSelection, expandSubgraph } from "./collapse";
import { SUBGRAPH_KIND } from "../nodes/subgraph";
import { cloneSnapshot, deepClone, type DocSnapshot } from "./snapshot";
import { resolveGraph, replaceGraph, subgraphAt } from "./subgraphNav";
import { nextId } from "./ids";

/** The fixed offset applied to each successive paste/duplicate of a selection. */
export const PASTE_OFFSET: Vec2 = { x: 32, y: 32 };

/**
 * Which editing level the canvas is showing (#46): the PIPELINE view (each pass
 * is a node) or the per-pass node graph (drill-in). The level only swaps which
 * graph the shared canvas renders — the document is unchanged.
 */
export type EditorLevel = "pipeline" | "pass";

/** A remembered React Flow viewport (pan + zoom) for one level. */
export interface ViewportState {
  x: number;
  y: number;
  zoom: number;
}

/** Maximum number of undo entries retained (older entries are discarded). */
const HISTORY_LIMIT = 200;

/** The current node/edge selection (ids into the active pass's graph). */
export interface Selection {
  nodeIds: string[];
  edgeIds: string[];
}

/** One row of the aggregate PROBLEMS list (#54): a diagnostic + its origin pass. */
export interface ProblemEntry {
  /** The pass this diagnostic came from (so the list can group + navigate). */
  passId: string;
  /** The pass's display name (for the problems list). */
  passName: string;
  /** The diagnostic itself (carries the offending node id + message). */
  diagnostic: Diagnostic;
}

/** The live compile-loop status the editor surfaces (#54). */
export interface CompileLoopStatus {
  /** The per-node diagnostics (the inspector + node badges read these by id). */
  diagnosticsByNode: Record<string, Diagnostic[]>;
  /** The aggregate problems list, in pipeline order. */
  problems: ProblemEntry[];
  /** Whether the whole pipeline is renderable. */
  valid: boolean;
  /**
   * The generated slang each pass emitted this compile (#55), keyed by pass id —
   * `null` when the pass currently does not compile (a graph pass with blocking
   * errors). This is the SAME source the live preview ran and the exporter would
   * embed; the read-only generated-code viewer reads it.
   */
  sourcesByPassId: Record<string, string | null>;
}

/**
 * The generated-code state for one pass (#55): the source from the LATEST compile
 * (`null` when the pass currently fails) plus the last source that DID compile, so
 * the read-only viewer can show last-good output with a "stale" marker instead of
 * a misleading blank when the graph is momentarily invalid.
 */
export interface PassSourceState {
  /** The latest compile's generated slang, or `null` if it failed this round. */
  current: string | null;
  /** The most recent NON-null generated slang for this pass (last-good). */
  lastGood: string | null;
}

export interface DocumentState {
  // ---- document ----
  project: Project;
  activePassId: string;

  // ---- navigation (#46) ----
  /** Whether the canvas shows the pipeline view or a per-pass graph. */
  level: EditorLevel;
  /**
   * Subgraph drill-in path (#57): the chain of `kind=="subgraph"` node ids the
   * editor has drilled into, from the pass graph downward. Empty == editing the
   * pass graph itself; non-empty == editing the interior of the last node's
   * subgraph body. Only meaningful when `level === "pass"`. Editor-only nav
   * state — never part of an undo snapshot (validated/reset on history jumps).
   */
  subgraphPath: string[];
  /**
   * Per-level remembered viewport (pan/zoom). Keyed by level for the pipeline,
   * and by pass id for each per-pass graph, so navigating back restores the
   * exact prior pan/zoom. Editor-only; never part of an undo snapshot.
   */
  viewports: { pipeline: ViewportState | null; passes: Record<string, ViewportState> };
  /**
   * Per-level remembered selection so drilling out and back restores it. The
   * pipeline selection is a pass id; each pass graph remembers its own node/edge
   * selection. Editor-only.
   */
  selections: { pipeline: string | null; passes: Record<string, Selection> };

  // ---- editor-only state (NOT part of an undo snapshot) ----
  selection: Selection;
  clipboard: Clipboard | null;
  /** Whether the document has unsaved edits since the last load/save/reset. */
  dirty: boolean;
  /**
   * Compile diagnostics keyed by the offending node id (#54 populates this from
   * each pass's `compile_graph` result). The inspector (#47) reads it read-only
   * to surface per-node problems; it is editor-only, never part of a snapshot.
   */
  diagnosticsByNode: Record<string, Diagnostic[]>;
  /**
   * The aggregate PROBLEMS list (#54): every compile diagnostic, tagged with the
   * pass it came from, in pipeline order. Drives the problems panel + a count
   * badge. Editor-only, replaced wholesale on each compile.
   */
  problems: ProblemEntry[];
  /**
   * Whether the whole pipeline is currently renderable (#54): `false` when any
   * pass failed to compile (cycle / type error → no source) so the editor can
   * flag that the preview is NOT reflecting the document. `null` before the first
   * compile completes. Editor-only.
   */
  pipelineValid: boolean | null;
  /** Whether a compile is in flight (#54) — the preview may lag the document. */
  compiling: boolean;
  /**
   * The generated slang per pass (#55), keyed by pass id, with last-good tracking.
   * Populated by the live compile loop's `setCompileStatus`; read by the read-only
   * generated-code viewer. A pass absent from this map has never compiled. This is
   * editor-only output state — never part of an undo snapshot.
   */
  sourcesByPassId: Record<string, PassSourceState>;

  // ---- history ----
  past: DocSnapshot[];
  future: DocSnapshot[];
  /**
   * Pre-edit baseline stashed by beginInteraction() and pushed by commit().
   * `null` outside a live interaction. Not part of any serialized state.
   */
  pendingBaseline: DocSnapshot | null;

  // ---- derived reads ----
  /** The graph of the currently-active pass (empty if the pass is opaque code). */
  activeGraph: () => Graph;

  // ---- mutations (each pushes exactly one history entry) ----
  addNode: (kind: string, position: Vec2, data?: Record<string, unknown>) => string;
  addEdge: (source: string, sourcePort: string, target: string, targetPort: string) => string | null;
  /**
   * Discrete edit of a node's free-form `data`: shallow-merges `patch` into the
   * node's `data` (a `null`/`undefined` patch VALUE deletes that key) and pushes
   * exactly one history entry. Used by the inspector for atomic edits (a select,
   * a checkbox) where one click = one undo entry. For coalesced text typing,
   * pair `beginNodeDataEdit()` + live `patchNodeData()` + `commit()` instead.
   */
  updateNodeData: (nodeId: string, patch: Record<string, unknown>) => void;
  /**
   * LIVE, non-committing merge of `patch` into a node's `data` (same merge rules
   * as `updateNodeData`) WITHOUT touching history — the inspector's per-keystroke
   * path during a coalesced text edit opened by `beginInteraction()` and closed
   * by `commit()`.
   */
  patchNodeData: (nodeId: string, patch: Record<string, unknown>) => void;
  moveNodes: (moves: Array<{ id: string; position: Vec2 }>) => void;
  removeSelection: () => void;
  paste: () => void;
  duplicate: () => void;

  // ---- clipboard (copy does NOT push history) ----
  copy: () => void;

  // ---- selection ----
  setSelection: (selection: Selection) => void;
  clearSelection: () => void;

  // ---- diagnostics + compile status (the live compile loop, #54, owns these) ----
  /** Replace the per-node diagnostics map (the live compile loop, #54, owns this). */
  setDiagnosticsByNode: (byNode: Record<string, Diagnostic[]>) => void;
  /**
   * Apply a completed compile's status in ONE update (#54): the per-node
   * diagnostics, the aggregate problems list, and the global validity flag — so
   * the inspector, the problems panel, and the status indicator stay consistent.
   * Clears `compiling`.
   */
  setCompileStatus: (status: CompileLoopStatus) => void;
  /** Mark a compile as in flight / settled (#54) — drives the "compiling" hint. */
  setCompiling: (compiling: boolean) => void;

  // ---- pipeline / navigation (#46) ----
  /**
   * Append a fresh graph-authored pass at the END of the pipeline and make it
   * active. Returns the new pass id. One undo entry.
   */
  addPass: (name?: string) => string;
  /**
   * Remove a pass. Index-based texture references (PassOutputN/PassFeedbackN)
   * and feedbackPass are remapped; references to the removed pass become a
   * dangling sentinel (see passOps). Removing the last pass is a no-op. If the
   * active pass is removed, the active pass falls back to a neighbour. One undo
   * entry.
   */
  removePass: (passId: string) => void;
  /**
   * Move a pass within the pipeline. Pass order IS the .slangp index, so index
   * references are remapped to keep the chain wired identically. One undo entry.
   */
  reorderPass: (from: number, to: number) => void;

  // ---- pass settings (#48) ----
  /**
   * Shallow-merge `patch` into a pass's `settings` (Pass.settings) as ONE
   * undoable, dirty-marking edit. A `patch` value of `undefined` is ignored
   * (use an explicit `null` to clear a setting back to "unset / engine
   * default"). Nested ScaleAxis objects are replaced wholesale by the caller.
   * No-op (no history entry) when the pass is unknown or nothing changes.
   */
  updatePassSettings: (passId: string, patch: Partial<PassSettings>) => void;
  /**
   * Set the project's global feedback pass index (Project.feedbackPass), or
   * `null` to clear it. One undo entry; no-op when unchanged.
   */
  setFeedbackPass: (index: number | null) => void;

  // ---- pass-source kind switching (#52) ----
  /**
   * Switch a pass to a WHOLE-PASS CODE pass holding `source` verbatim (opaque,
   * never decomposed into node-IR). Its node graph (if any) is discarded. One
   * undo entry; no-op when already whole-pass code with the same source.
   */
  setPassToWholePassCode: (passId: string, source: string) => void;
  /**
   * Switch a pass back to a GRAPH pass (carrying `graph`, default empty). Its
   * whole-pass source is discarded. One undo entry; no-op when already a graph.
   */
  setPassToGraph: (passId: string, graph?: Graph) => void;
  /**
   * LIVE, non-committing replace of a whole-pass code pass's verbatim source
   * (the code-editor's per-keystroke path) — pair with `beginInteraction()` +
   * `commit()` to coalesce a typing burst into one undo entry. No-op when the
   * pass is not whole-pass code.
   */
  patchWholePassSource: (passId: string, source: string) => void;

  /** Set (or clear) the selected pass in the pipeline view. */
  setPipelineSelection: (passId: string | null) => void;
  /** Switch the canvas to the pipeline view (remembering the pass viewport). */
  showPipeline: () => void;
  /** Drill into a pass's graph (remembering the pipeline viewport + selection). */
  openPass: (passId: string) => void;

  /**
   * Insert a library item's already-instantiated payload (#59) into the active
   * graph at `position`. The payload MUST already carry fresh interior ids
   * (mint them with `instantiateLibraryItem`); this action only mints the ONE
   * wrapping node id and drops the node in. A `subgraph` payload drops in as a
   * collapsed, drill-in-editable `kind=="subgraph"` node whose `data` is the
   * Subgraph body; a `node` payload drops in as that node (its id is replaced
   * with a fresh one so the action is self-contained). One undo entry; selects
   * the inserted node. Returns the inserted node's id. Going through the store
   * makes history + the debounced compile loop fire automatically.
   */
  insertLibraryPayload: (payload: LibraryPayload, position: Vec2) => string;

  // ---- subgraph collapse / expand / drill-in (#57) ----
  /**
   * Collapse the current node selection into ONE named `kind=="subgraph"` node:
   * the boundary (edges crossing the selection) becomes typed boundary ports,
   * the interior nodes/edges move into the new node's `data` Subgraph, and the
   * crossing parent edges are rewired to the new node's boundary ports. One undo
   * entry; no-op when fewer than one node is selected. Selects the new node.
   */
  collapseSelection: (name: string) => void;
  /**
   * Expand a `kind=="subgraph"` node back to its interior (fresh ids),
   * reconnecting boundary ports to the exterior endpoints on the parent edges —
   * the inverse of `collapseSelection`. One undo entry; no-op when `nodeId` is
   * not a subgraph node. Selects the restored interior nodes.
   */
  expandSubgraphNode: (nodeId: string) => void;
  /**
   * Drill INTO a subgraph node's interior body (#57): push `nodeId` onto the
   * drill-in path and edit its interior as a graph (boundary ports rendered as
   * in/out terminals). Remembers the current level's selection. No-op when
   * `nodeId` is not a subgraph node in the active graph.
   */
  openSubgraph: (nodeId: string) => void;
  /**
   * Drill OUT one subgraph level (#57): pop the last entry off the drill-in path
   * (back to the parent graph), or to the pipeline when already at the pass
   * graph. Remembers/restores the per-level selection.
   */
  closeSubgraph: () => void;
  /** Remember the current level's React Flow viewport (called on pan/zoom). */
  setViewport: (viewport: ViewportState) => void;
  /** The remembered viewport for the current level, or null if none yet. */
  currentViewport: () => ViewportState | null;

  // ---- React Flow change plumbing (live, NON-committing) ----
  applyNodeChanges: (changes: NodeChange[]) => void;
  applyEdgeChanges: (changes: EdgeChange[]) => void;

  /**
   * Stash the current document as the baseline for a coalesced interaction
   * (e.g. node drag). The following live `applyNodeChanges` edits do not touch
   * history; a single `commit()` on drag-stop pushes this baseline.
   */
  beginInteraction: () => void;
  /**
   * Close a coalesced interaction: push the stashed baseline onto the undo
   * stack as ONE entry (only if the document actually changed) and clear redo.
   * No-op if no interaction is open or nothing changed (e.g. a click with no
   * drag).
   */
  commit: () => void;

  // ---- history ----
  undo: () => void;
  redo: () => void;
  canUndo: () => boolean;
  canRedo: () => boolean;

  // ---- serialization ----
  toSnapshot: () => DocSnapshot;
  fromSnapshot: (snap: DocSnapshot, options?: { resetHistory?: boolean }) => void;

  /** Replace the whole project (e.g. on file load); clears history + selection. */
  loadProject: (project: Project, activePassId?: string) => void;
  /** Reset to a fresh single-pass project. */
  reset: () => void;
}

/**
 * Read the ACTIVE graph out of a project: the pass graph when `subgraphPath` is
 * empty, else the interior body of the subgraph node chain `subgraphPath`
 * addresses (drill-in, #57). An empty graph for opaque/code passes.
 */
function graphOf(project: Project, activePassId: string, subgraphPath: string[]): Graph {
  if (subgraphPath.length === 0) {
    const pass = project.passes.find((p) => p.id === activePassId);
    if (pass && pass.source.kind === "graph") {
      return pass.source.graph;
    }
    return emptyGraph();
  }
  return resolveGraph(project, activePassId, subgraphPath);
}

/**
 * Return a NEW project with the ACTIVE graph (addressed by `subgraphPath`)
 * replaced by `next`. Threads the edit back through any subgraph nodes so
 * React/zustand see fresh references the whole way; untouched siblings shared.
 */
function withGraph(
  project: Project,
  activePassId: string,
  subgraphPath: string[],
  next: Graph,
): Project {
  return replaceGraph(project, activePassId, subgraphPath, next);
}

/**
 * The key under which the active graph's per-level selection/viewport is
 * remembered (#46/#57): the active pass id at the pass level, or the deepest
 * drilled-in subgraph node id when editing a subgraph interior. Node ids and
 * pass ids never collide (distinct id prefixes), so one map serves both.
 */
function navKey(activePassId: string, subgraphPath: string[]): string {
  return subgraphPath.length > 0 ? subgraphPath[subgraphPath.length - 1]! : activePassId;
}

/**
 * Apply a shallow `patch` to a node's `data`, returning a NEW node (and a new
 * `data` object). A patch value of `undefined` deletes that key (so the inspector
 * can drop a now-irrelevant field, e.g. when a Const switches variant). The node
 * is left untouched (same reference) when `data` does not actually change.
 */
function patchNode(node: Node, patch: Record<string, unknown>): Node {
  const next: Record<string, unknown> = { ...node.data };
  let changed = false;
  for (const [key, value] of Object.entries(patch)) {
    if (value === undefined) {
      if (key in next) {
        delete next[key];
        changed = true;
      }
    } else if (next[key] !== value) {
      next[key] = value;
      changed = true;
    }
  }
  return changed ? { ...node, data: next } : node;
}

/** Replace one node's `data` (via `patchNode`) inside a graph, sharing the rest. */
function withNodeData(
  graph: Graph,
  nodeId: string,
  patch: Record<string, unknown>,
): Graph {
  let touched = false;
  const nodes = graph.nodes.map((n) => {
    if (n.id !== nodeId) {
      return n;
    }
    const next = patchNode(n, patch);
    if (next !== n) {
      touched = true;
    }
    return next;
  });
  return touched ? { ...graph, nodes } : graph;
}

/**
 * Shallow-merge a settings `patch` into a pass's `PassSettings`, returning a NEW
 * object (or the same reference when nothing changes). A patch value of
 * `undefined` is skipped — callers pass an explicit `null` to clear a key back
 * to "unset". Object-valued keys (the ScaleAxis axes) are compared by deep
 * equality so a structurally-identical replacement is a no-op.
 */
function mergeSettings(
  settings: PassSettings,
  patch: Partial<PassSettings>,
): PassSettings {
  const next: PassSettings = { ...settings };
  let changed = false;
  for (const [key, value] of Object.entries(patch) as [keyof PassSettings, unknown][]) {
    if (value === undefined) {
      continue;
    }
    const prev = next[key];
    if (prev === value) {
      continue;
    }
    if (
      typeof prev === "object" &&
      prev !== null &&
      typeof value === "object" &&
      value !== null &&
      JSON.stringify(prev) === JSON.stringify(value)
    ) {
      continue;
    }
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (next as any)[key] = value;
    changed = true;
  }
  return changed ? next : settings;
}

/**
 * Fold a compile's per-pass sources into the store's last-good-tracking map (#55).
 * Every pass present in `next` records its `current` source; its `lastGood` is the
 * new source when non-null, else the previously remembered last-good. Passes absent
 * from `next` (e.g. removed) are dropped, so the map never leaks stale pass ids.
 */
function mergePassSources(
  prev: Record<string, PassSourceState>,
  next: Record<string, string | null>,
): Record<string, PassSourceState> {
  const out: Record<string, PassSourceState> = {};
  for (const [passId, current] of Object.entries(next)) {
    const lastGood = current ?? prev[passId]?.lastGood ?? null;
    out[passId] = { current, lastGood };
  }
  return out;
}

const initialProject = makeProject();
const initialActivePassId = initialProject.passes[0]!.id;

export const useDocumentStore = create<DocumentState>((set, get) => {
  /** Snapshot the live document (deep-cloned) for the history stacks. */
  function snapshot(): DocSnapshot {
    const { project, activePassId } = get();
    return { project: deepClone(project), activePassId };
  }

  /**
   * After a history jump (undo/redo) the restored project may no longer contain
   * the pass we were drilled into. Fall back to the pipeline view so the canvas
   * never renders a stale/empty pass graph.
   */
  function levelFor(project: Project, activePassId: string, level: EditorLevel): EditorLevel {
    if (level === "pass" && !project.passes.some((p) => p.id === activePassId)) {
      return "pipeline";
    }
    return level;
  }

  /**
   * Trim `subgraphPath` to the longest valid prefix in `project` (a history jump
   * may have removed a subgraph node we were drilled into). Each step must name a
   * subgraph node in the graph the prefix-so-far resolves to.
   */
  function validPath(project: Project, activePassId: string, path: string[]): string[] {
    const valid: string[] = [];
    for (const nodeId of path) {
      const graph = graphOf(project, activePassId, valid);
      if (!subgraphAt(graph, nodeId)) {
        break;
      }
      valid.push(nodeId);
    }
    return valid;
  }

  /** Append `before` to the undo stack, capping its length. */
  function pushPast(before: DocSnapshot): DocSnapshot[] {
    const past = [...get().past, before];
    if (past.length > HISTORY_LIMIT) {
      past.splice(0, past.length - HISTORY_LIMIT);
    }
    return past;
  }

  /**
   * Apply a graph transform as ONE undoable, dirty-marking commit: snapshot the
   * pre-edit document, transform the active graph, push history, clear redo.
   */
  function mutateGraph(transform: (graph: Graph) => Graph): void {
    const before = snapshot();
    const { project, activePassId, subgraphPath } = get();
    const nextGraph = transform(graphOf(project, activePassId, subgraphPath));
    set({
      project: withGraph(project, activePassId, subgraphPath, nextGraph),
      past: pushPast(before),
      future: [],
      dirty: true,
    });
  }

  return {
    project: initialProject,
    activePassId: initialActivePassId,
    level: "pipeline" as EditorLevel,
    subgraphPath: [],
    viewports: { pipeline: null, passes: {} },
    selections: { pipeline: null, passes: {} },
    selection: { nodeIds: [], edgeIds: [] },
    clipboard: null,
    dirty: false,
    diagnosticsByNode: {},
    problems: [],
    pipelineValid: null,
    compiling: false,
    sourcesByPassId: {},
    past: [],
    future: [],
    pendingBaseline: null,

    activeGraph: () => graphOf(get().project, get().activePassId, get().subgraphPath),

    addNode: (kind, position, data) => {
      const node = makeNode(kind, position, data);
      mutateGraph((g) => ({ ...g, nodes: [...g.nodes, node] }));
      return node.id;
    },

    addEdge: (source, sourcePort, target, targetPort) => {
      const g = get().activeGraph();
      // Reject self-loops and a second connection into the same target port.
      if (source === target) {
        return null;
      }
      const dup = g.edges.some(
        (e) => e.target === target && e.targetPort === targetPort,
      );
      if (dup) {
        return null;
      }
      const edge = makeEdge(source, sourcePort, target, targetPort);
      mutateGraph((graph) => ({ ...graph, edges: [...graph.edges, edge] }));
      return edge.id;
    },

    updateNodeData: (nodeId, patch) => {
      const g = get().activeGraph();
      // Skip the snapshot + history churn when the patch is a no-op.
      if (withNodeData(g, nodeId, patch) === g) {
        return;
      }
      mutateGraph((graph) => withNodeData(graph, nodeId, patch));
    },

    patchNodeData: (nodeId, patch) => {
      set((s) => ({
        project: withGraph(
          s.project,
          s.activePassId,
          s.subgraphPath,
          withNodeData(graphOf(s.project, s.activePassId, s.subgraphPath), nodeId, patch),
        ),
        dirty: true,
      }));
    },

    moveNodes: (moves) => {
      if (moves.length === 0) {
        return;
      }
      const byId = new Map(moves.map((m) => [m.id, m.position] as const));
      mutateGraph((g) => ({
        ...g,
        nodes: g.nodes.map((n) =>
          byId.has(n.id) ? { ...n, position: { ...byId.get(n.id)! } } : n,
        ),
      }));
    },

    removeSelection: () => {
      const { selection } = get();
      if (selection.nodeIds.length === 0 && selection.edgeIds.length === 0) {
        return;
      }
      const nodeIds = new Set(selection.nodeIds);
      const edgeIds = new Set(selection.edgeIds);
      mutateGraph((g) => ({
        nodes: g.nodes.filter((n) => !nodeIds.has(n.id)),
        // Drop explicitly-selected edges AND any edge whose endpoint is gone.
        edges: g.edges.filter(
          (e) => !edgeIds.has(e.id) && !nodeIds.has(e.source) && !nodeIds.has(e.target),
        ),
      }));
      set({ selection: { nodeIds: [], edgeIds: [] } });
    },

    copy: () => {
      const { selection } = get();
      if (selection.nodeIds.length === 0) {
        return;
      }
      const g = get().activeGraph();
      set({ clipboard: captureClipboard(g.nodes, g.edges, selection.nodeIds) });
    },

    paste: () => {
      const clip = get().clipboard;
      if (!clip || clip.nodes.length === 0) {
        return;
      }
      const fresh = instantiateClipboard(clip, PASTE_OFFSET);
      mutateGraph((g) => ({
        nodes: [...g.nodes, ...fresh.nodes],
        edges: [...g.edges, ...fresh.edges],
      }));
      // Select the freshly pasted nodes/edges so a follow-up paste cascades.
      set({
        selection: {
          nodeIds: fresh.nodes.map((n) => n.id),
          edgeIds: fresh.edges.map((e) => e.id),
        },
      });
    },

    duplicate: () => {
      const { selection } = get();
      if (selection.nodeIds.length === 0) {
        return;
      }
      const g = get().activeGraph();
      const clip = captureClipboard(g.nodes, g.edges, selection.nodeIds);
      const fresh = instantiateClipboard(clip, PASTE_OFFSET);
      mutateGraph((graph) => ({
        nodes: [...graph.nodes, ...fresh.nodes],
        edges: [...graph.edges, ...fresh.edges],
      }));
      set({
        selection: {
          nodeIds: fresh.nodes.map((n) => n.id),
          edgeIds: fresh.edges.map((e) => e.id),
        },
      });
    },

    setSelection: (selection) => set({ selection }),
    clearSelection: () => set({ selection: { nodeIds: [], edgeIds: [] } }),

    setDiagnosticsByNode: (byNode) => set({ diagnosticsByNode: byNode }),

    setCompileStatus: (status) =>
      set((s) => ({
        diagnosticsByNode: status.diagnosticsByNode,
        problems: status.problems,
        pipelineValid: status.valid,
        compiling: false,
        // Merge the new per-pass sources, carrying forward each pass's last-good
        // source so the read-only viewer (#55) can fall back to it (with a stale
        // marker) when this round produced no source for that pass.
        sourcesByPassId: mergePassSources(s.sourcesByPassId, status.sourcesByPassId),
      })),

    setCompiling: (compiling) => set({ compiling }),

    addPass: (name) => {
      const before = snapshot();
      const { project } = get();
      const pass = makePass(name ?? `Pass ${project.passes.length + 1}`);
      set({
        project: addPass(project, pass),
        activePassId: pass.id,
        // The new pass starts at its top-level graph.
        subgraphPath: [],
        past: pushPast(before),
        future: [],
        dirty: true,
      });
      return pass.id;
    },

    removePass: (passId) => {
      const { project } = get();
      if (project.passes.length <= 1) {
        return;
      }
      if (!project.passes.some((p) => p.id === passId)) {
        return;
      }
      const before = snapshot();
      const removedIndex = project.passes.findIndex((p) => p.id === passId);
      const nextProject = removePass(project, passId);
      // If the active pass was removed, fall back to the neighbour that now sits
      // at the removed slot (or the new last pass).
      const priorActivePassId = get().activePassId;
      let activePassId = priorActivePassId;
      if (activePassId === passId) {
        const fallbackIndex = Math.min(removedIndex, nextProject.passes.length - 1);
        activePassId = nextProject.passes[fallbackIndex]!.id;
      }
      // Drop the removed pass's remembered viewport/selection.
      const { viewports, selections } = get();
      const passViewports = { ...viewports.passes };
      delete passViewports[passId];
      const passSelections = { ...selections.passes };
      delete passSelections[passId];
      set((s) => ({
        project: nextProject,
        activePassId,
        // If the active pass changed, any drill-in into it is no longer valid.
        subgraphPath: activePassId === priorActivePassId ? s.subgraphPath : [],
        viewports: { ...viewports, passes: passViewports },
        selections: { ...selections, passes: passSelections },
        past: pushPast(before),
        future: [],
        dirty: true,
      }));
    },

    reorderPass: (from, to) => {
      const { project } = get();
      const n = project.passes.length;
      if (from < 0 || from >= n || to < 0 || to >= n || from === to) {
        return;
      }
      const before = snapshot();
      set({
        project: reorderPass(project, from, to),
        past: pushPast(before),
        future: [],
        dirty: true,
      });
    },

    updatePassSettings: (passId, patch) => {
      const { project } = get();
      const pass = project.passes.find((p) => p.id === passId);
      if (!pass) {
        return;
      }
      const nextSettings = mergeSettings(pass.settings, patch);
      if (nextSettings === pass.settings) {
        return;
      }
      const before = snapshot();
      set({
        project: {
          ...project,
          passes: project.passes.map((p) =>
            p.id === passId ? { ...p, settings: nextSettings } : p,
          ),
        },
        past: pushPast(before),
        future: [],
        dirty: true,
      });
    },

    setFeedbackPass: (index) => {
      const { project } = get();
      if (project.feedbackPass === index) {
        return;
      }
      const before = snapshot();
      set({
        project: { ...project, feedbackPass: index },
        past: pushPast(before),
        future: [],
        dirty: true,
      });
    },

    setPassToWholePassCode: (passId, source) => {
      const { project } = get();
      const pass = project.passes.find((p) => p.id === passId);
      if (!pass) {
        return;
      }
      const next = passToWholePassCode(pass, source);
      if (next === pass) {
        return;
      }
      const before = snapshot();
      set({
        project: {
          ...project,
          passes: project.passes.map((p) => (p.id === passId ? next : p)),
        },
        // The pass no longer has a node graph — clear any node selection + any
        // drill-in into it.
        selection:
          get().activePassId === passId ? { nodeIds: [], edgeIds: [] } : get().selection,
        subgraphPath: get().activePassId === passId ? [] : get().subgraphPath,
        past: pushPast(before),
        future: [],
        dirty: true,
      });
    },

    setPassToGraph: (passId, graph) => {
      const { project } = get();
      const pass = project.passes.find((p) => p.id === passId);
      if (!pass) {
        return;
      }
      const next = passToGraph(pass, graph);
      if (next === pass) {
        return;
      }
      const before = snapshot();
      set({
        project: {
          ...project,
          passes: project.passes.map((p) => (p.id === passId ? next : p)),
        },
        past: pushPast(before),
        future: [],
        dirty: true,
      });
    },

    patchWholePassSource: (passId, source) => {
      set((s) => {
        const pass = s.project.passes.find((p) => p.id === passId);
        if (!pass) {
          return {};
        }
        const next = setWholePassSource(pass, source);
        if (next === pass) {
          return {};
        }
        return {
          project: {
            ...s.project,
            passes: s.project.passes.map((p) => (p.id === passId ? next : p)),
          },
          dirty: true,
        };
      });
    },

    setPipelineSelection: (passId) => {
      set((s) => ({ selections: { ...s.selections, pipeline: passId } }));
    },

    showPipeline: () => {
      const { level, activePassId, subgraphPath, selection } = get();
      if (level === "pipeline") {
        return;
      }
      // Remember the current graph's selection (pass or subgraph interior).
      const here = navKey(activePassId, subgraphPath);
      set((s) => ({
        level: "pipeline",
        subgraphPath: [],
        selections: {
          ...s.selections,
          passes: { ...s.selections.passes, [here]: selection },
        },
        selection: { nodeIds: [], edgeIds: [] },
      }));
    },

    openPass: (passId) => {
      const { project } = get();
      if (!project.passes.some((p) => p.id === passId)) {
        return;
      }
      // Restore the pass's remembered selection (empty if first visit).
      const remembered = get().selections.passes[passId] ?? {
        nodeIds: [],
        edgeIds: [],
      };
      set((s) => ({
        level: "pass",
        activePassId: passId,
        // Opening a pass starts at its top-level graph (clear any drill-in).
        subgraphPath: [],
        // Remember which pass was selected in the pipeline.
        selections: { ...s.selections, pipeline: passId },
        selection: remembered,
      }));
    },

    insertLibraryPayload: (payload, position) => {
      // Mint the ONE wrapping/placed-node id (the payload's interior ids are
      // already fresh from instantiateLibraryItem). For a subgraph payload the
      // node's `data` IS the Subgraph body; for a node payload we re-id the node
      // so the action is self-contained regardless of how the payload was built.
      const nodeId = nextId("node");
      const node: Node =
        payload.kind === "subgraph"
          ? {
              id: nodeId,
              kind: SUBGRAPH_KIND,
              position,
              data: payload.subgraph as unknown as Record<string, unknown>,
            }
          : { ...payload.node, id: nodeId, position };
      mutateGraph((g) => ({ ...g, nodes: [...g.nodes, node] }));
      set({ selection: { nodeIds: [nodeId], edgeIds: [] } });
      return nodeId;
    },

    collapseSelection: (name) => {
      const { selection } = get();
      if (selection.nodeIds.length === 0) {
        return;
      }
      const before = snapshot();
      const { project, activePassId, subgraphPath } = get();
      const graph = graphOf(project, activePassId, subgraphPath);
      const result = collapseSelection(graph, selection.nodeIds, name, nextId);
      if (!result) {
        return;
      }
      set({
        project: withGraph(project, activePassId, subgraphPath, result.graph),
        past: pushPast(before),
        future: [],
        dirty: true,
        // Select the freshly-created collapsed node.
        selection: { nodeIds: [result.subgraphNodeId], edgeIds: [] },
      });
    },

    expandSubgraphNode: (nodeId) => {
      const before = snapshot();
      const { project, activePassId, subgraphPath } = get();
      const graph = graphOf(project, activePassId, subgraphPath);
      const next = expandSubgraph(graph, nodeId, nextId);
      if (!next) {
        return;
      }
      // The restored interior nodes are the ones absent from the original graph.
      const priorIds = new Set(graph.nodes.map((n) => n.id));
      const restoredIds = next.nodes.filter((n) => !priorIds.has(n.id)).map((n) => n.id);
      set({
        project: withGraph(project, activePassId, subgraphPath, next),
        past: pushPast(before),
        future: [],
        dirty: true,
        selection: { nodeIds: restoredIds, edgeIds: [] },
      });
    },

    openSubgraph: (nodeId) => {
      const { project, activePassId, subgraphPath, selection } = get();
      const graph = graphOf(project, activePassId, subgraphPath);
      if (!subgraphAt(graph, nodeId)) {
        return;
      }
      // Remember the current graph's selection before drilling in.
      const here = navKey(activePassId, subgraphPath);
      const nextPath = [...subgraphPath, nodeId];
      const remembered = get().selections.passes[navKey(activePassId, nextPath)] ?? {
        nodeIds: [],
        edgeIds: [],
      };
      set((s) => ({
        level: "pass",
        subgraphPath: nextPath,
        selections: {
          ...s.selections,
          passes: { ...s.selections.passes, [here]: selection },
        },
        selection: remembered,
      }));
    },

    closeSubgraph: () => {
      const { activePassId, subgraphPath, selection } = get();
      if (subgraphPath.length === 0) {
        // Already at the pass graph — fall back to the pipeline.
        get().showPipeline();
        return;
      }
      const here = navKey(activePassId, subgraphPath);
      const nextPath = subgraphPath.slice(0, -1);
      const parentKey = navKey(activePassId, nextPath);
      const remembered = get().selections.passes[parentKey] ?? {
        nodeIds: [],
        edgeIds: [],
      };
      set((s) => ({
        subgraphPath: nextPath,
        selections: {
          ...s.selections,
          passes: { ...s.selections.passes, [here]: selection },
        },
        selection: remembered,
      }));
    },

    setViewport: (viewport) => {
      set((s) => {
        if (s.level === "pipeline") {
          return { viewports: { ...s.viewports, pipeline: viewport } };
        }
        const here = navKey(s.activePassId, s.subgraphPath);
        return {
          viewports: {
            ...s.viewports,
            passes: { ...s.viewports.passes, [here]: viewport },
          },
        };
      });
    },

    currentViewport: () => {
      const { level, viewports, activePassId, subgraphPath } = get();
      return level === "pipeline"
        ? viewports.pipeline
        : (viewports.passes[navKey(activePassId, subgraphPath)] ?? null);
    },

    applyNodeChanges: (changes) => {
      // Map RF node changes onto the document WITHOUT pushing history — this is
      // the live, per-pointermove path. Drag-stop is committed via commit().
      const g = get().activeGraph();
      const rfNodes = g.nodes.map((n) => ({
        id: n.id,
        position: { ...n.position },
        data: {} as Record<string, unknown>,
        type: n.kind,
      }));
      const nextRf = applyNodeChanges(changes, rfNodes);
      const posById = new Map(nextRf.map((n) => [n.id, n.position] as const));
      const survivingIds = new Set(nextRf.map((n) => n.id));
      const nextNodes = g.nodes
        .filter((n) => survivingIds.has(n.id))
        .map((n) => {
          const p = posById.get(n.id);
          return p ? { ...n, position: { x: p.x, y: p.y } } : n;
        });
      // Removals also prune now-dangling edges.
      const nextEdges =
        nextNodes.length === g.nodes.length
          ? g.edges
          : g.edges.filter((e) => survivingIds.has(e.source) && survivingIds.has(e.target));
      set((s) => ({
        project: withGraph(s.project, s.activePassId, s.subgraphPath, {
          nodes: nextNodes,
          edges: nextEdges,
        }),
      }));
    },

    applyEdgeChanges: (changes) => {
      const g = get().activeGraph();
      const rfEdges = g.edges.map((e) => ({
        id: e.id,
        source: e.source,
        target: e.target,
        sourceHandle: e.sourcePort,
        targetHandle: e.targetPort,
      }));
      const nextRf = applyEdgeChanges(changes, rfEdges);
      const survivingIds = new Set(nextRf.map((e) => e.id));
      const nextEdges = g.edges.filter((e) => survivingIds.has(e.id));
      set((s) => ({
        project: withGraph(s.project, s.activePassId, s.subgraphPath, {
          nodes: g.nodes,
          edges: nextEdges,
        }),
      }));
    },

    beginInteraction: () => {
      // Only stash once per interaction; a multi-node drag fires drag-start per
      // node, but the baseline must be the state BEFORE the first one.
      if (get().pendingBaseline === null) {
        set({ pendingBaseline: snapshot() });
      }
    },

    commit: () => {
      const baseline = get().pendingBaseline;
      if (!baseline) {
        return;
      }
      const current = snapshot();
      // No-op interaction (e.g. a click with no movement): discard the baseline
      // without polluting history.
      if (JSON.stringify(baseline) === JSON.stringify(current)) {
        set({ pendingBaseline: null });
        return;
      }
      set({
        past: pushPast(baseline),
        future: [],
        pendingBaseline: null,
        dirty: true,
      });
    },

    undo: () => {
      const { past } = get();
      if (past.length === 0) {
        return;
      }
      const previous = past[past.length - 1]!;
      const current = snapshot();
      set((s) => {
        const level = levelFor(previous.project, previous.activePassId, s.level);
        // A history jump may have removed a subgraph node we were inside — trim
        // the drill-in path to its longest still-valid prefix (empty at pipeline).
        const subgraphPath =
          level === "pass"
            ? validPath(previous.project, previous.activePassId, s.subgraphPath)
            : [];
        return {
          project: deepClone(previous.project),
          activePassId: previous.activePassId,
          level,
          subgraphPath,
          past: past.slice(0, -1),
          future: [...s.future, current],
          // Selection may reference deleted nodes after undo — clear it.
          selection: { nodeIds: [], edgeIds: [] },
          pendingBaseline: null,
          dirty: true,
        };
      });
    },

    redo: () => {
      const { future } = get();
      if (future.length === 0) {
        return;
      }
      const next = future[future.length - 1]!;
      const current = snapshot();
      set((s) => {
        const level = levelFor(next.project, next.activePassId, s.level);
        const subgraphPath =
          level === "pass"
            ? validPath(next.project, next.activePassId, s.subgraphPath)
            : [];
        return {
          project: deepClone(next.project),
          activePassId: next.activePassId,
          level,
          subgraphPath,
          past: [...s.past, current],
          future: future.slice(0, -1),
          selection: { nodeIds: [], edgeIds: [] },
          pendingBaseline: null,
          dirty: true,
        };
      });
    },

    canUndo: () => get().past.length > 0,
    canRedo: () => get().future.length > 0,

    toSnapshot: () => snapshot(),

    fromSnapshot: (snap, options) => {
      const cloned = cloneSnapshot(snap);
      set((s) => {
        const level = levelFor(cloned.project, cloned.activePassId, s.level);
        const subgraphPath =
          level === "pass"
            ? validPath(cloned.project, cloned.activePassId, s.subgraphPath)
            : [];
        return {
          project: cloned.project,
          activePassId: cloned.activePassId,
          level,
          subgraphPath,
          selection: { nodeIds: [], edgeIds: [] },
          pendingBaseline: null,
          past: options?.resetHistory ? [] : s.past,
          future: options?.resetHistory ? [] : s.future,
        };
      });
    },

    loadProject: (project, activePassId) => {
      const firstGraphPass = project.passes.find((p) => p.source.kind === "graph");
      const active = activePassId ?? firstGraphPass?.id ?? project.passes[0]?.id ?? "";
      set({
        project: deepClone(project),
        activePassId: active,
        level: "pipeline",
        subgraphPath: [],
        viewports: { pipeline: null, passes: {} },
        selections: { pipeline: null, passes: {} },
        selection: { nodeIds: [], edgeIds: [] },
        clipboard: null,
        diagnosticsByNode: {},
        problems: [],
        pipelineValid: null,
        compiling: false,
        sourcesByPassId: {},
        past: [],
        future: [],
        pendingBaseline: null,
        dirty: false,
      });
    },

    reset: () => {
      const fresh = makeProject();
      set({
        project: fresh,
        activePassId: fresh.passes[0]!.id,
        level: "pipeline",
        subgraphPath: [],
        viewports: { pipeline: null, passes: {} },
        selections: { pipeline: null, passes: {} },
        selection: { nodeIds: [], edgeIds: [] },
        clipboard: null,
        diagnosticsByNode: {},
        problems: [],
        pipelineValid: null,
        compiling: false,
        sourcesByPassId: {},
        past: [],
        future: [],
        pendingBaseline: null,
        dirty: false,
      });
    },
  };
});

/** The placeholder node kind, re-exported for convenience. */
export { PLACEHOLDER_KIND };
