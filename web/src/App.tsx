import "./App.css";
import { useCompileLoop } from "./compile/useCompileLoop";
import { EditorCanvas } from "./editor/EditorCanvas";
import { ExportDialog } from "./export/ExportDialog";
import { ErrorBoundary } from "./feedback/ErrorBoundary";
import { Toasts } from "./feedback/Toasts";
import { useEngineEvents } from "./feedback/useEngineEvents";
import { HelpButton } from "./help/HelpButton";
import { StartScreen } from "./onboarding/StartScreen";
import { useOnboardingStore } from "./onboarding/onboardingStore";
import { PanelLayout } from "./panels/PanelLayout";
import { ConfirmDialog } from "./session/ConfirmDialog";
import { FileMenu } from "./session/FileMenu";
import { basename } from "./session/paths";
import { useSession } from "./session/useSession";
import { useDocumentStore } from "./store/documentStore";
import { PreviewCanvas } from "./preview/PreviewCanvas";

// Phase 5: the node-editor shell. The React Flow canvas (with palette, toolbar,
// status bar, undo/redo, copy/paste) is driven by the document store; #54 closes
// the live edit → compile → preview loop via useCompileLoop.
export default function App() {
  const projectName = useDocumentStore((s) => s.project.name);
  const dirty = useDocumentStore((s) => s.dirty);
  const currentProjectPath = useDocumentStore((s) => s.currentProjectPath);

  // First-run start screen (#66): shown until the user picks a starting point
  // (New / Open / Import / Open example) or restores recovered work this launch.
  const started = useOnboardingStore((s) => s.started);

  // The debounced live compile loop (#54): document edits → graphToIr →
  // compile_graph per pass → node-keyed diagnostics + the generated chain pushed
  // to the engine preview. Runs for the app's lifetime.
  useCompileLoop();

  // Engine status/error events (#62): the render thread's typed status (live /
  // last-good / stopped) + render/compile errors → the store + non-blocking toasts.
  useEngineEvents();

  // Session lifecycle (#63): mirror dirty to the backend, autosave recovery,
  // guard the window close, and offer recovery on launch.
  useSession();

  return (
    <div className="app">
      <header className="app__titlebar">
        ShaderBuilder <span className="app__phase">editor</span>
        <FileMenu />
        <HelpButton />
        <span className="app__project">
          {/* `*` marks unsaved edits (#63). */}
          {dirty ? "* " : ""}
          {projectName}
          {currentProjectPath ? (
            <span className="app__path" title={currentProjectPath}>
              {" "}
              — {basename(currentProjectPath)}
            </span>
          ) : null}
        </span>
      </header>

      {/* An error boundary keeps a render-time exception from white-screening the
          whole window (#62): the editor + preview subtree fails to a recoverable
          screen while the title bar + document store stay intact. */}
      <ErrorBoundary label="Editor">
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
      </ErrorBoundary>

      {/* Non-blocking toast stack (#62): transient engine/render/IO failures. */}
      <Toasts />

      {/* Blocking save/discard/cancel + recovery modal (#63). */}
      <ConfirmDialog />

      {/* The export-bundle dialog (#64): destination + name + validation gate. */}
      <ExportDialog />

      {/* First-run START SCREEN (#66): a full-window welcome cover shown until the
          user picks New / Open / Import / Open example (or recovers work). Rendered
          last so it sits ABOVE the editor, which stays mounted underneath to keep
          the compile/preview/session hooks alive. */}
      {started ? null : <StartScreen />}
    </div>
  );
}
