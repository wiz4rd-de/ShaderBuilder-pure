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
  type IsValidConnection,
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
import { judgeConnection } from "../nodes/portTypeChecking";
import { isSubgraphNode } from "../nodes/subgraph";
import { useDocumentStore } from "../store/documentStore";
import { WholePassEditor } from "../wholePass/WholePassEditor";
import { EditorStatusBar } from "./EditorStatusBar";
import { EditorToolbar } from "./EditorToolbar";
import { toRfGraph } from "./graphAdapter";
import { partitionNodeChanges } from "./nodeMeasureCache";
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
  const subgraphPath = useDocumentStore((s) => s.subgraphPath);
  // The active graph's nav key: the pass id, or the deepest drilled-in subgraph
  // node id. Drives the per-graph remembered viewport AND the canvas remount key
  // so drilling into a subgraph (same pass) gives a fresh React Flow instance.
  const navKey = subgraphPath.length > 0 ? subgraphPath[subgraphPath.length - 1]! : activePassId;
  const rememberedViewport = useDocumentStore((s) => s.viewports.passes[navKey] ?? null);
  // React Flow MEASURED-dimension cache (NOT part of the document). React Flow 12
  // keeps a node hidden until it has measured its on-screen size, and in
  // CONTROLLED mode it reads `node.measured` off the `nodes` prop on every render
  // (see `adoptUserNodes` in @xyflow/system). The document has no node dimensions,
  // so without this cache every render looked "unmeasured" to React Flow → it
  // re-measured → fired a `dimensions` change → `applyNodeChanges` rebuilt the
  // nodes (still unmeasured) → re-measure … an infinite update loop (React error
  // #185) that also left every node permanently invisible. We stash each node's
  // measured size here (keyed by id) and merge it back into the derived nodes, so
  // React Flow sees a stable measurement and the loop never starts.
  const measured = useRef<Map<string, { width: number; height: number }>>(new Map());
  const [measuredVersion, setMeasuredVersion] = useState(0);

  const base = useMemo(() => toRfGraph(graph, selection), [graph, selection]);
  const nodes = useMemo(
    () =>
      base.nodes.map((n) => {
        const m = measured.current.get(n.id);
        return m ? { ...n, measured: m } : n;
      }),
    [base, measuredVersion],
  );
  const edges = base.edges;

  const applyNodeChanges = useDocumentStore((s) => s.applyNodeChanges);
  const applyEdgeChanges = useDocumentStore((s) => s.applyEdgeChanges);
  const addEdge = useDocumentStore((s) => s.addEdge);
  const setSelection = useDocumentStore((s) => s.setSelection);
  const setViewport = useDocumentStore((s) => s.setViewport);
  const beginInteraction = useDocumentStore((s) => s.beginInteraction);
  const commit = useDocumentStore((s) => s.commit);
  const openSubgraph = useDocumentStore((s) => s.openSubgraph);

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
    (changes: NodeChange[]) => {
      // Capture React Flow's measurements into the side cache above instead of
      // round-tripping them through the document store — that round-trip (the doc
      // carries no dimensions) is what caused the infinite re-measure loop. All
      // STRUCTURAL changes (position / select / remove) still flow to the store.
      const { structural, measureChanged } = partitionNodeChanges(changes, measured.current);
      if (structural.length > 0) {
        applyNodeChanges(structural);
      }
      if (measureChanged) {
        setMeasuredVersion((v) => v + 1);
      }
    },
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
      // The edge is type-checked at DRAG time by `isValidConnection` below (#65),
      // so by the time onConnect fires the wire is already known compatible. The
      // store's `addEdge` re-applies the SAME `judgeConnection` verdict as a
      // belt-and-braces guard (and still rejects self-loops / dup-target ports);
      // any residual type-mismatch the checker can only see with full operand
      // inference still surfaces as a live compile diagnostic.
      addEdge(conn.source, conn.sourceHandle ?? "", conn.target, conn.targetHandle ?? "");
    },
    [addEdge],
  );

  // DRAG-TIME connection legality (#65): React Flow consults this on every
  // candidate connection while dragging a wire. We resolve both endpoint port
  // types from the live node descriptors + data and apply the SAME edge-legality
  // predicate the IR uses (proven by the cross-language parity golden), so an
  // incompatible drop (e.g. a vec4 sampler output into a tightened vec2 coord, or
  // a sampler2D into a float) is REFUSED before it ever reaches compile_graph.
  // A wire we cannot judge (unknown kind / dropped port) is permitted — the
  // authoritative compiler still runs. Returning false also drives React Flow's
  // per-handle `valid`/`invalid` connection-state classes (the drag affordance).
  const isValidConnection = useCallback<IsValidConnection>(
    (conn) => {
      if (!conn.source || !conn.target || conn.source === conn.target) {
        return false;
      }
      return judgeConnection(
        graph,
        conn.source,
        conn.sourceHandle ?? "",
        conn.target,
        conn.targetHandle ?? "",
      ).legal;
    },
    [graph],
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

  // Double-click a subgraph node to drill into its interior (#57).
  const onNodeDoubleClick = useCallback(
    (_event: React.MouseEvent, rfNode: { id: string }) => {
      const target = graph.nodes.find((n) => n.id === rfNode.id);
      if (target && isSubgraphNode(target)) {
        openSubgraph(rfNode.id);
      }
    },
    [graph, openSubgraph],
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
          key={navKey}
          nodes={nodes}
          edges={edges}
          nodeTypes={NODE_TYPES}
          onNodesChange={onNodesChange}
          onEdgesChange={onEdgesChange}
          onConnect={onConnect}
          isValidConnection={isValidConnection}
          onSelectionChange={onSelectionChange}
          onNodeDoubleClick={onNodeDoubleClick}
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
