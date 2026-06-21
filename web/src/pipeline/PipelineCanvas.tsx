// The PIPELINE-view canvas (#46). Renders Project.passes as a derived React Flow
// graph (one Pass node per pass; edges = cross-pass texture bindings). It reuses
// the SAME canvas surface as the per-pass editor — EditorCanvas swaps between
// this and the node graph by `level`.
//
// Interactions:
//   * click a pass        → select it (remembered as selections.pipeline).
//   * double-click a pass → drill into its per-pass graph (openPass).
//   * pan/zoom            → remembered per-level (setViewport) so back restores it.
// Reorder/add/remove are driven by the PipelineToolbar (pass order = .slangp index).
//
// SELECTION IS ONE-WAY (store → nodes). The pipeline node's `selected` flag is
// DERIVED from `selections.pipeline`, and selection is changed only by explicit
// clicks (onNodeClick / onPaneClick). We deliberately do NOT use React Flow's
// `onSelectionChange` here: that fed React Flow's own selection back into the
// store, which re-derived the nodes, which made React Flow re-emit the selection,
// which … looped into "Maximum update depth exceeded" (React #185) on returning
// to the pipeline with a pass already selected. One-way binding can't ping-pong.
import {
  Background,
  Controls,
  ReactFlow,
  useReactFlow,
  type Node as RfNode,
  type Viewport,
} from "@xyflow/react";
import { useCallback, useEffect, useMemo, useRef } from "react";

import { useDocumentStore } from "../store/documentStore";
import { toPipelineGraph } from "./pipelineGraph";
import { PIPELINE_NODE_TYPES } from "./pipelineNodeTypes";

/** Stable across renders so it never churns React Flow's StoreUpdater. */
const PRO_OPTIONS = { hideAttribution: true } as const;

export function PipelineCanvas() {
  const project = useDocumentStore((s) => s.project);
  const selectedPassId = useDocumentStore((s) => s.selections.pipeline);
  const setPipelineSelection = useDocumentStore((s) => s.setPipelineSelection);
  const openPass = useDocumentStore((s) => s.openPass);
  const setViewport = useDocumentStore((s) => s.setViewport);
  const rememberedViewport = useDocumentStore((s) => s.viewports.pipeline);

  const { setViewport: applyRfViewport } = useReactFlow();

  const { nodes, edges } = useMemo(
    () => toPipelineGraph(project, selectedPassId),
    [project, selectedPassId],
  );

  // Restore the remembered viewport once on mount (level switch remounts this).
  const restored = useRef(false);
  useEffect(() => {
    if (!restored.current && rememberedViewport) {
      restored.current = true;
      applyRfViewport(rememberedViewport);
    }
  }, [rememberedViewport, applyRfViewport]);

  // Click drives selection (one-way: store → derived `selected`). See file header.
  const onNodeClick = useCallback(
    (_event: React.MouseEvent, node: RfNode) => setPipelineSelection(node.id),
    [setPipelineSelection],
  );
  const onPaneClick = useCallback(
    () => setPipelineSelection(null),
    [setPipelineSelection],
  );

  const onNodeDoubleClick = useCallback(
    (_event: React.MouseEvent, node: RfNode) => {
      openPass(node.id);
    },
    [openPass],
  );

  const onMoveEnd = useCallback(
    (_event: unknown, viewport: Viewport) => setViewport(viewport),
    [setViewport],
  );

  return (
    <ReactFlow
      nodes={nodes}
      edges={edges}
      nodeTypes={PIPELINE_NODE_TYPES}
      onNodeClick={onNodeClick}
      onPaneClick={onPaneClick}
      onNodeDoubleClick={onNodeDoubleClick}
      onMoveEnd={onMoveEnd}
      nodesConnectable={false}
      nodesDraggable={false}
      fitView={!rememberedViewport}
      proOptions={PRO_OPTIONS}
    >
      <Background />
      <Controls />
    </ReactFlow>
  );
}
