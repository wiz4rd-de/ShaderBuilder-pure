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

import type { Graph } from "../bindings/Graph";
import type { Project } from "../bindings/Project";
import type { Vec2 } from "../bindings/Vec2";
import {
  captureClipboard,
  instantiateClipboard,
  type Clipboard,
} from "./clipboard";
import { emptyGraph, makeEdge, makeNode, makeProject, PLACEHOLDER_KIND } from "./factories";
import { cloneSnapshot, deepClone, type DocSnapshot } from "./snapshot";

/** The fixed offset applied to each successive paste/duplicate of a selection. */
export const PASTE_OFFSET: Vec2 = { x: 32, y: 32 };

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

  // ---- editor-only state (NOT part of an undo snapshot) ----
  selection: Selection;
  clipboard: Clipboard | null;
  /** Whether the document has unsaved edits since the last load/save/reset. */
  dirty: boolean;

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
  moveNodes: (moves: Array<{ id: string; position: Vec2 }>) => void;
  removeSelection: () => void;
  paste: () => void;
  duplicate: () => void;

  // ---- clipboard (copy does NOT push history) ----
  copy: () => void;

  // ---- selection ----
  setSelection: (selection: Selection) => void;
  clearSelection: () => void;

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

const initialProject = makeProject();
const initialActivePassId = initialProject.passes[0]!.id;

export const useDocumentStore = create<DocumentState>((set, get) => {
  /** Snapshot the live document (deep-cloned) for the history stacks. */
  function snapshot(): DocSnapshot {
    const { project, activePassId } = get();
    return { project: deepClone(project), activePassId };
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
    selection: { nodeIds: [], edgeIds: [] },
    clipboard: null,
    dirty: false,
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
        selection: { nodeIds: [], edgeIds: [] },
        clipboard: null,
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
        selection: { nodeIds: [], edgeIds: [] },
        clipboard: null,
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
