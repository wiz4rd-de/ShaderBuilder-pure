// The editor status bar (#45, #46): at the pipeline level it shows pass counts +
// the selected pass; in a pass graph it shows node/edge totals + selection. A
// dirty indicator is always shown. Reads derived counts off the document store.
import { useDocumentStore } from "../store/documentStore";

export function EditorStatusBar() {
  const level = useDocumentStore((s) => s.level);
  const dirty = useDocumentStore((s) => s.dirty);

  const passCount = useDocumentStore((s) => s.project.passes.length);
  const selectedPassId = useDocumentStore((s) => s.selections.pipeline);

  const selectedNodes = useDocumentStore((s) => s.selection.nodeIds.length);
  const selectedEdges = useDocumentStore((s) => s.selection.edgeIds.length);
  const nodeCount = useDocumentStore((s) => s.activeGraph().nodes.length);
  const edgeCount = useDocumentStore((s) => s.activeGraph().edges.length);

  const selectionCount = selectedNodes + selectedEdges;

  return (
    <footer className="editor__statusbar" aria-label="Editor status">
      {level === "pipeline" ? (
        <>
          <span className="editor__status-item" data-testid="status-counts">
            {passCount} {passCount === 1 ? "pass" : "passes"}
          </span>
          <span className="editor__status-item" data-testid="status-selection">
            {selectedPassId === null ? "No pass selected" : "1 pass selected"}
          </span>
        </>
      ) : (
        <>
          <span className="editor__status-item" data-testid="status-counts">
            {nodeCount} {nodeCount === 1 ? "node" : "nodes"}, {edgeCount}{" "}
            {edgeCount === 1 ? "edge" : "edges"}
          </span>
          <span className="editor__status-item" data-testid="status-selection">
            {selectionCount === 0
              ? "No selection"
              : `${selectionCount} selected (${selectedNodes} ${
                  selectedNodes === 1 ? "node" : "nodes"
                }, ${selectedEdges} ${selectedEdges === 1 ? "edge" : "edges"})`}
          </span>
        </>
      )}
      <span
        className={`editor__status-item editor__dirty${dirty ? " editor__dirty--on" : ""}`}
        data-testid="status-dirty"
      >
        {dirty ? "Unsaved changes" : "Saved"}
      </span>
    </footer>
  );
}
