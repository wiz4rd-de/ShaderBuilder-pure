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
import {
  Background,
  Controls,
  ReactFlow,
  useReactFlow,
  type Node as RfNode,
  type OnSelectionChangeParams,
  type Viewport,
} from "@xyflow/react";
import { useCallback, useEffect, useMemo, useRef } from "react";

import { useDocumentStore } from "../store/documentStore";
import { toPipelineGraph } from "./pipelineGraph";
import { PIPELINE_NODE_TYPES } from "./pipelineNodeTypes";

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

  const onSelectionChange = useCallback(
    (params: OnSelectionChangeParams) => {
      setPipelineSelection(params.nodes[0]?.id ?? null);
    },
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
      onSelectionChange={onSelectionChange}
      onNodeDoubleClick={onNodeDoubleClick}
      onMoveEnd={onMoveEnd}
      nodesConnectable={false}
      nodesDraggable={false}
      fitView={!rememberedViewport}
      proOptions={{ hideAttribution: true }}
    >
      <Background />
      <Controls />
    </ReactFlow>
  );
}
