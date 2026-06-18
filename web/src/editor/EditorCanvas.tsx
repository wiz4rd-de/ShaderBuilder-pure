// The node editor canvas (#45, #46). React Flow in controlled mode: nodes/edges
// are DERIVED from the document store, and all interactions write back through it.
//
// Two-level editing (#46): the SAME canvas surface renders either the PIPELINE
// view (each pass = a node) or a drilled-in pass's per-pass node graph, switched
// by the store's `level`. A breadcrumb returns to the pipeline; per-level pan/zoom
// and selection are remembered so navigating back restores them.
//
// History semantics (pass graph):
//   * Discrete edits (palette insert, connect, paste, delete) push one entry.
//   * A node/selection drag is COALESCED: beginInteraction() on drag-start,
//     live position updates during drag, commit() once on drag-stop.
import {
  Background,
  Controls,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
  type Connection,
  type EdgeChange,
  type NodeChange,
  type OnSelectionChangeParams,
  type Viewport,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { PipelineBreadcrumb } from "../pipeline/PipelineBreadcrumb";
import { PipelineCanvas } from "../pipeline/PipelineCanvas";
import { PipelineToolbar } from "../pipeline/PipelineToolbar";
import { NODE_TYPES } from "../nodes/nodeTypes";
import { useDocumentStore } from "../store/documentStore";
import { WholePassEditor } from "../wholePass/WholePassEditor";
import { EditorStatusBar } from "./EditorStatusBar";
import { EditorToolbar } from "./EditorToolbar";
import { toRfGraph } from "./graphAdapter";
import { NodePaletteMenu } from "./NodePaletteMenu";
import { useEditorShortcuts } from "./useEditorShortcuts";

interface PaletteAnchor {
  screen: { x: number; y: number };
  graphPosition: { x: number; y: number };
}

/** The per-pass node graph (drilled-in level). */
function PassGraph() {
  const { screenToFlowPosition, setViewport: applyRfViewport } = useReactFlow();

  // Derive RF arrays from the store (single source of truth). Subscribing to
  // graph + selection means any store mutation re-renders the canvas.
  const graph = useDocumentStore((s) => s.activeGraph());
  const selection = useDocumentStore((s) => s.selection);
  const activePassId = useDocumentStore((s) => s.activePassId);
  const rememberedViewport = useDocumentStore((s) => s.viewports.passes[s.activePassId] ?? null);
  const { nodes, edges } = useMemo(() => toRfGraph(graph, selection), [graph, selection]);

  const applyNodeChanges = useDocumentStore((s) => s.applyNodeChanges);
  const applyEdgeChanges = useDocumentStore((s) => s.applyEdgeChanges);
  const addEdge = useDocumentStore((s) => s.addEdge);
  const setSelection = useDocumentStore((s) => s.setSelection);
  const setViewport = useDocumentStore((s) => s.setViewport);
  const beginInteraction = useDocumentStore((s) => s.beginInteraction);
  const commit = useDocumentStore((s) => s.commit);

  const [palette, setPalette] = useState<PaletteAnchor | null>(null);

  // Restore this pass's remembered viewport once on mount (drill-in remounts).
  const restored = useRef(false);
  useEffect(() => {
    if (!restored.current && rememberedViewport) {
      restored.current = true;
      applyRfViewport(rememberedViewport);
    }
  }, [rememberedViewport, applyRfViewport]);

  const onNodesChange = useCallback(
    (changes: NodeChange[]) => applyNodeChanges(changes),
    [applyNodeChanges],
  );
  const onEdgesChange = useCallback(
    (changes: EdgeChange[]) => applyEdgeChanges(changes),
    [applyEdgeChanges],
  );

  const onConnect = useCallback(
    (conn: Connection) => {
      if (!conn.source || !conn.target) {
        return;
      }
      // CONNECTION VALIDITY (#54): the AUTHORITATIVE type-checking + connection
      // validity live in the Rust `ir` crate (Phase 4) and are reported back to
      // the editor as compile diagnostics (inline node badges + the Problems
      // panel) by the live compile loop (compile/useCompileLoop.ts). We accept any
      // structurally-valid edge here (the store still rejects self-loops and a
      // double-connection into one target port) and let the checker flag a TYPE
      // mismatch after the fact. The STRICT in-editor type-checked rule — rejecting
      // an incompatible edge at DRAG TIME (e.g. blocking a vec4→float drop) — is
      // DEFERRED to Phase 7; that needs port-type resolution on the React Flow
      // handles, which is out of scope for the Phase-5 compile loop.
      addEdge(conn.source, conn.sourceHandle ?? "", conn.target, conn.targetHandle ?? "");
    },
    [addEdge],
  );

  // Mirror RF's selection into the store so toolbar/status/keyboard agree.
  const onSelectionChange = useCallback(
    (params: OnSelectionChangeParams) => {
      setSelection({
        nodeIds: params.nodes.map((n) => n.id),
        edgeIds: params.edges.map((e) => e.id),
      });
    },
    [setSelection],
  );

  // Drag coalescing: one undo entry per drag, committed on stop.
  const onNodeDragStart = useCallback(() => beginInteraction(), [beginInteraction]);
  const onNodeDragStop = useCallback(() => commit(), [commit]);
  const onSelectionDragStart = useCallback(() => beginInteraction(), [beginInteraction]);
  const onSelectionDragStop = useCallback(() => commit(), [commit]);

  const onMoveEnd = useCallback(
    (_event: unknown, viewport: Viewport) => setViewport(viewport),
    [setViewport],
  );

  const onPaneContextMenu = useCallback(
    (event: React.MouseEvent | MouseEvent) => {
      event.preventDefault();
      const graphPosition = screenToFlowPosition({ x: event.clientX, y: event.clientY });
      setPalette({ screen: { x: event.clientX, y: event.clientY }, graphPosition });
    },
    [screenToFlowPosition],
  );

  const closePalette = useCallback(() => setPalette(null), []);

  return (
    <>
      <EditorToolbar />
      <div className="editor__canvas" onClick={palette ? closePalette : undefined}>
        <ReactFlow
          key={activePassId}
          nodes={nodes}
          edges={edges}
          nodeTypes={NODE_TYPES}
          onNodesChange={onNodesChange}
          onEdgesChange={onEdgesChange}
          onConnect={onConnect}
          onSelectionChange={onSelectionChange}
          onNodeDragStart={onNodeDragStart}
          onNodeDragStop={onNodeDragStop}
          onSelectionDragStart={onSelectionDragStart}
          onSelectionDragStop={onSelectionDragStop}
          onMoveEnd={onMoveEnd}
          onPaneContextMenu={onPaneContextMenu}
          selectionOnDrag
          panOnDrag={[1, 2]}
          fitView={!rememberedViewport}
          proOptions={{ hideAttribution: true }}
        >
          <Background />
          <Controls />
        </ReactFlow>
        {palette ? (
          <NodePaletteMenu
            screen={palette.screen}
            graphPosition={palette.graphPosition}
            onClose={closePalette}
          />
        ) : null}
      </div>
    </>
  );
}

function CanvasInner() {
  useEditorShortcuts();
  const level = useDocumentStore((s) => s.level);
  // An opaque whole-pass code pass (#52) has no node graph — it shows the
  // pass-level code editor at the pass level instead of the React Flow canvas.
  const activeIsWholePass = useDocumentStore((s) => {
    const pass = s.project.passes.find((p) => p.id === s.activePassId);
    return pass?.source.kind === "wholePassCode";
  });

  return (
    <div className="editor__canvas-host">
      <PipelineBreadcrumb />
      {level === "pipeline" ? (
        <>
          <PipelineToolbar />
          <div className="editor__canvas">
            <PipelineCanvas />
          </div>
        </>
      ) : activeIsWholePass ? (
        <WholePassEditor />
      ) : (
        <PassGraph />
      )}
      <EditorStatusBar />
    </div>
  );
}

/** The editor surface, providing the React Flow context its children need. */
export function EditorCanvas() {
  return (
    <ReactFlowProvider>
      <CanvasInner />
    </ReactFlowProvider>
  );
}
