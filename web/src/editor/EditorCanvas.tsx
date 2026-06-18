// The node editor canvas (#45). React Flow in controlled mode: nodes/edges are
// DERIVED from the document store, and all interactions write back through it.
// History semantics:
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
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { useCallback, useMemo, useState } from "react";

import { useDocumentStore } from "../store/documentStore";
import { EditorStatusBar } from "./EditorStatusBar";
import { EditorToolbar } from "./EditorToolbar";
import { toRfGraph } from "./graphAdapter";
import { NodePaletteMenu } from "./NodePaletteMenu";
import { useEditorShortcuts } from "./useEditorShortcuts";

interface PaletteAnchor {
  screen: { x: number; y: number };
  graphPosition: { x: number; y: number };
}

function CanvasInner() {
  useEditorShortcuts();
  const { screenToFlowPosition } = useReactFlow();

  // Derive RF arrays from the store (single source of truth). Subscribing to
  // graph + selection means any store mutation re-renders the canvas.
  const graph = useDocumentStore((s) => s.activeGraph());
  const selection = useDocumentStore((s) => s.selection);
  const { nodes, edges } = useMemo(() => toRfGraph(graph, selection), [graph, selection]);

  const applyNodeChanges = useDocumentStore((s) => s.applyNodeChanges);
  const applyEdgeChanges = useDocumentStore((s) => s.applyEdgeChanges);
  const addEdge = useDocumentStore((s) => s.addEdge);
  const setSelection = useDocumentStore((s) => s.setSelection);
  const beginInteraction = useDocumentStore((s) => s.beginInteraction);
  const commit = useDocumentStore((s) => s.commit);

  const [palette, setPalette] = useState<PaletteAnchor | null>(null);

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
    <div className="editor__canvas-host">
      <EditorToolbar />
      <div className="editor__canvas" onClick={palette ? closePalette : undefined}>
        <ReactFlow
          nodes={nodes}
          edges={edges}
          onNodesChange={onNodesChange}
          onEdgesChange={onEdgesChange}
          onConnect={onConnect}
          onSelectionChange={onSelectionChange}
          onNodeDragStart={onNodeDragStart}
          onNodeDragStop={onNodeDragStop}
          onSelectionDragStart={onSelectionDragStart}
          onSelectionDragStop={onSelectionDragStop}
          onPaneContextMenu={onPaneContextMenu}
          selectionOnDrag
          panOnDrag={[1, 2]}
          fitView
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
