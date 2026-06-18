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
import type { Node } from "../bindings/Node";
import type { PassSettings } from "../bindings/PassSettings";
import type { Project } from "../bindings/Project";
import type { Vec2 } from "../bindings/Vec2";
import { addPass, removePass, reorderPass } from "../pipeline/passOps";
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
import { cloneSnapshot, deepClone, type DocSnapshot } from "./snapshot";

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

export interface DocumentState {
  // ---- document ----
  project: Project;
  activePassId: string;

  // ---- navigation (#46) ----
  /** Whether the canvas shows the pipeline view or a per-pass graph. */
  level: EditorLevel;
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

  // ---- diagnostics (read-only hook for #54) ----
  /** Replace the per-node diagnostics map (the live compile loop, #54, owns this). */
  setDiagnosticsByNode: (byNode: Record<string, Diagnostic[]>) => void;

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

  /** Set (or clear) the selected pass in the pipeline view. */
  setPipelineSelection: (passId: string | null) => void;
  /** Switch the canvas to the pipeline view (remembering the pass viewport). */
  showPipeline: () => void;
  /** Drill into a pass's graph (remembering the pipeline viewport + selection). */
  openPass: (passId: string) => void;
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

/** Read the active pass's graph out of a project, or an empty graph. */
function graphOf(project: Project, activePassId: string): Graph {
  const pass = project.passes.find((p) => p.id === activePassId);
  if (pass && pass.source.kind === "graph") {
    return pass.source.graph;
  }
  return emptyGraph();
}

/**
 * Return a NEW project with the active pass's graph replaced by `next`. The
 * project (and the touched pass + its source) are shallow-cloned so React/zustand
 * see a fresh reference; untouched passes are shared.
 */
function withGraph(project: Project, activePassId: string, next: Graph): Project {
  return {
    ...project,
    passes: project.passes.map((p) => {
      if (p.id !== activePassId || p.source.kind !== "graph") {
        return p;
      }
      return { ...p, source: { ...p.source, graph: next } };
    }),
  };
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
    const { project, activePassId } = get();
    const nextGraph = transform(graphOf(project, activePassId));
    set({
      project: withGraph(project, activePassId, nextGraph),
      past: pushPast(before),
      future: [],
      dirty: true,
    });
  }

  return {
    project: initialProject,
    activePassId: initialActivePassId,
    level: "pipeline" as EditorLevel,
    viewports: { pipeline: null, passes: {} },
    selections: { pipeline: null, passes: {} },
    selection: { nodeIds: [], edgeIds: [] },
    clipboard: null,
    dirty: false,
    diagnosticsByNode: {},
    past: [],
    future: [],
    pendingBaseline: null,

    activeGraph: () => graphOf(get().project, get().activePassId),

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
          withNodeData(graphOf(s.project, s.activePassId), nodeId, patch),
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

    addPass: (name) => {
      const before = snapshot();
      const { project } = get();
      const pass = makePass(name ?? `Pass ${project.passes.length + 1}`);
      set({
        project: addPass(project, pass),
        activePassId: pass.id,
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
      let { activePassId } = get();
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
      set({
        project: nextProject,
        activePassId,
        viewports: { ...viewports, passes: passViewports },
        selections: { ...selections, passes: passSelections },
        past: pushPast(before),
        future: [],
        dirty: true,
      });
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

    setPipelineSelection: (passId) => {
      set((s) => ({ selections: { ...s.selections, pipeline: passId } }));
    },

    showPipeline: () => {
      const { level, activePassId, selection } = get();
      if (level === "pipeline") {
        return;
      }
      // Remember the pass-graph selection before switching out.
      set((s) => ({
        level: "pipeline",
        selections: {
          ...s.selections,
          passes: { ...s.selections.passes, [activePassId]: selection },
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
        // Remember which pass was selected in the pipeline.
        selections: { ...s.selections, pipeline: passId },
        selection: remembered,
      }));
    },

    setViewport: (viewport) => {
      set((s) => {
        if (s.level === "pipeline") {
          return { viewports: { ...s.viewports, pipeline: viewport } };
        }
        return {
          viewports: {
            ...s.viewports,
            passes: { ...s.viewports.passes, [s.activePassId]: viewport },
          },
        };
      });
    },

    currentViewport: () => {
      const { level, viewports, activePassId } = get();
      return level === "pipeline"
        ? viewports.pipeline
        : (viewports.passes[activePassId] ?? null);
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
        project: withGraph(s.project, s.activePassId, { nodes: nextNodes, edges: nextEdges }),
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
        project: withGraph(s.project, s.activePassId, { nodes: g.nodes, edges: nextEdges }),
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
      set((s) => ({
        project: deepClone(previous.project),
        activePassId: previous.activePassId,
        level: levelFor(previous.project, previous.activePassId, s.level),
        past: past.slice(0, -1),
        future: [...s.future, current],
        // Selection may reference deleted nodes after undo — clear it.
        selection: { nodeIds: [], edgeIds: [] },
        pendingBaseline: null,
        dirty: true,
      }));
    },

    redo: () => {
      const { future } = get();
      if (future.length === 0) {
        return;
      }
      const next = future[future.length - 1]!;
      const current = snapshot();
      set((s) => ({
        project: deepClone(next.project),
        activePassId: next.activePassId,
        level: levelFor(next.project, next.activePassId, s.level),
        past: [...s.past, current],
        future: future.slice(0, -1),
        selection: { nodeIds: [], edgeIds: [] },
        pendingBaseline: null,
        dirty: true,
      }));
    },

    canUndo: () => get().past.length > 0,
    canRedo: () => get().future.length > 0,

    toSnapshot: () => snapshot(),

    fromSnapshot: (snap, options) => {
      const cloned = cloneSnapshot(snap);
      set((s) => ({
        project: cloned.project,
        activePassId: cloned.activePassId,
        selection: { nodeIds: [], edgeIds: [] },
        pendingBaseline: null,
        past: options?.resetHistory ? [] : s.past,
        future: options?.resetHistory ? [] : s.future,
      }));
    },

    loadProject: (project, activePassId) => {
      const firstGraphPass = project.passes.find((p) => p.source.kind === "graph");
      const active = activePassId ?? firstGraphPass?.id ?? project.passes[0]?.id ?? "";
      set({
        project: deepClone(project),
        activePassId: active,
        level: "pipeline",
        viewports: { pipeline: null, passes: {} },
        selections: { pipeline: null, passes: {} },
        selection: { nodeIds: [], edgeIds: [] },
        clipboard: null,
        diagnosticsByNode: {},
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
        viewports: { pipeline: null, passes: {} },
        selections: { pipeline: null, passes: {} },
        selection: { nodeIds: [], edgeIds: [] },
        clipboard: null,
        diagnosticsByNode: {},
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
