import "./App.css";
import { EditorCanvas } from "./editor/EditorCanvas";
import { PanelLayout } from "./panels/PanelLayout";
import { useDocumentStore } from "./store/documentStore";
import { PreviewCanvas } from "./preview/PreviewCanvas";

// Phase 5: the node-editor shell. The React Flow canvas (with palette, toolbar,
// status bar, undo/redo, copy/paste) is driven by the document store; the node
// taxonomy, inspectors, pipeline view, and live compile arrive in later issues.
export default function App() {
  const projectName = useDocumentStore((s) => s.project.name);

  return (
    <div className="app">
      <header className="app__titlebar">
        ShaderBuilder <span className="app__phase">Phase 5 editor</span>
        <span className="app__project">{projectName}</span>
      </header>

      <div className="app__body">
        {/* Editor region — the React Flow node graph (Architecture §A). */}
        <main className="editor" aria-label="Node editor">
          <EditorCanvas />
        </main>

        {/* Right region — the tabbed panel layout (#48): inspector + the
            engine-driving panels above the always-visible preview pane. */}
        <div className="app__right" aria-label="Panels">
          <PanelLayout />

          {/* Preview region — the wgpu frame stream blits into a <canvas> here. */}
          <aside className="preview" aria-label="Preview">
            <div className="preview__header">Preview</div>
            <div className="preview__pane">
              <PreviewCanvas />
            </div>
          </aside>
        </div>
      </div>
    </div>
  );
}
