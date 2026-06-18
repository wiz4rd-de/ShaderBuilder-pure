// The editor toolbar (#45): add-node, undo/redo, and clipboard buttons. All
// actions go through the document store so they share the keyboard path's
// history semantics exactly.
import { isSubgraphNode } from "../nodes/subgraph";
import { useDocumentStore } from "../store/documentStore";
import { PLACEHOLDER_KIND } from "../store/factories";

export function EditorToolbar() {
  const canUndo = useDocumentStore((s) => s.past.length > 0);
  const canRedo = useDocumentStore((s) => s.future.length > 0);
  const hasSelection = useDocumentStore((s) => s.selection.nodeIds.length > 0);
  const hasClipboard = useDocumentStore((s) => (s.clipboard?.nodes.length ?? 0) > 0);
  // Expand is offered only when the selection is EXACTLY one subgraph node.
  const selectedSubgraphId = useDocumentStore((s) => {
    if (s.selection.nodeIds.length !== 1) {
      return null;
    }
    const id = s.selection.nodeIds[0]!;
    const node = s.activeGraph().nodes.find((n) => n.id === id);
    return node && isSubgraphNode(node) ? id : null;
  });

  const addNode = useDocumentStore((s) => s.addNode);
  const undo = useDocumentStore((s) => s.undo);
  const redo = useDocumentStore((s) => s.redo);
  const copy = useDocumentStore((s) => s.copy);
  const paste = useDocumentStore((s) => s.paste);
  const duplicate = useDocumentStore((s) => s.duplicate);
  const removeSelection = useDocumentStore((s) => s.removeSelection);
  const collapseSelection = useDocumentStore((s) => s.collapseSelection);
  const expandSubgraphNode = useDocumentStore((s) => s.expandSubgraphNode);

  const onCollapse = () => {
    // Prompt for a name (falls back to a sensible default if cancelled/blank or
    // when no prompt is available, e.g. in a test harness).
    const name =
      typeof window !== "undefined" && typeof window.prompt === "function"
        ? window.prompt("Subgraph name", "Subgraph")
        : "Subgraph";
    if (name === null) {
      return; // user cancelled
    }
    collapseSelection(name.trim().length > 0 ? name.trim() : "Subgraph");
  };

  return (
    <div className="editor__toolbar" role="toolbar" aria-label="Editor actions">
      <button
        type="button"
        onClick={() => addNode(PLACEHOLDER_KIND, { x: 120, y: 80 })}
        title="Add a placeholder node"
      >
        Add node
      </button>
      <span className="editor__toolbar-sep" aria-hidden="true" />
      <button type="button" onClick={undo} disabled={!canUndo} title="Undo (Ctrl+Z)">
        Undo
      </button>
      <button type="button" onClick={redo} disabled={!canRedo} title="Redo (Ctrl+Shift+Z)">
        Redo
      </button>
      <span className="editor__toolbar-sep" aria-hidden="true" />
      <button type="button" onClick={copy} disabled={!hasSelection} title="Copy (Ctrl+C)">
        Copy
      </button>
      <button type="button" onClick={paste} disabled={!hasClipboard} title="Paste (Ctrl+V)">
        Paste
      </button>
      <button
        type="button"
        onClick={duplicate}
        disabled={!hasSelection}
        title="Duplicate (Ctrl+D)"
      >
        Duplicate
      </button>
      <button
        type="button"
        onClick={removeSelection}
        disabled={!hasSelection}
        title="Delete (Del)"
      >
        Delete
      </button>
      <span className="editor__toolbar-sep" aria-hidden="true" />
      <button
        type="button"
        onClick={onCollapse}
        disabled={!hasSelection}
        title="Collapse selection into a subgraph"
      >
        Collapse
      </button>
      <button
        type="button"
        onClick={() => selectedSubgraphId && expandSubgraphNode(selectedSubgraphId)}
        disabled={selectedSubgraphId === null}
        title="Expand the selected subgraph"
      >
        Expand
      </button>
    </div>
  );
}
