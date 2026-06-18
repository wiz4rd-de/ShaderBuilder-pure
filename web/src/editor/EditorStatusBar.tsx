// The editor status bar (#45): selection count, node/edge totals, and a dirty
// indicator. Reads derived counts off the document store.
import { useDocumentStore } from "../store/documentStore";

export function EditorStatusBar() {
  const selectedNodes = useDocumentStore((s) => s.selection.nodeIds.length);
  const selectedEdges = useDocumentStore((s) => s.selection.edgeIds.length);
  const dirty = useDocumentStore((s) => s.dirty);
  const nodeCount = useDocumentStore((s) => s.activeGraph().nodes.length);
  const edgeCount = useDocumentStore((s) => s.activeGraph().edges.length);

  const selectionCount = selectedNodes + selectedEdges;

  return (
    <footer className="editor__statusbar" aria-label="Editor status">
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
      <span
        className={`editor__status-item editor__dirty${dirty ? " editor__dirty--on" : ""}`}
        data-testid="status-dirty"
      >
        {dirty ? "Unsaved changes" : "Saved"}
      </span>
    </footer>
  );
}
