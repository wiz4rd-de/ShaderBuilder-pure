import { Background, Controls, ReactFlow, type Edge, type Node } from "@xyflow/react";
import "@xyflow/react/dist/style.css";

import "./App.css";
import { EMPTY_PROJECT } from "./model";
import { PreviewCanvas } from "./preview/PreviewCanvas";

// Phase 0: an empty editor surface. The node taxonomy, inspectors, pipeline
// view, and live compile arrive in Phase 5 — this only reserves the canvas.
const initialNodes: Node[] = [];
const initialEdges: Edge[] = [];

export default function App() {
  // Typed against the generated core-model bindings — drift is a compile error.
  const project = EMPTY_PROJECT;

  return (
    <div className="app">
      <header className="app__titlebar">
        ShaderBuilder <span className="app__phase">Phase 0 shell</span>
        <span className="app__project">{project.name}</span>
      </header>

      <div className="app__body">
        {/* Editor region — the React Flow node graph (Architecture §A). */}
        <main className="editor" aria-label="Node editor">
          <ReactFlow nodes={initialNodes} edges={initialEdges} fitView proOptions={{ hideAttribution: true }}>
            <Background />
            <Controls />
          </ReactFlow>
        </main>

        {/* Preview region — the wgpu frame stream blits into a <canvas> here in #13. */}
        <aside className="preview" aria-label="Preview">
          <div className="preview__header">Preview</div>
          <div className="preview__pane">
            <PreviewCanvas />
          </div>
        </aside>
      </div>
    </div>
  );
}
