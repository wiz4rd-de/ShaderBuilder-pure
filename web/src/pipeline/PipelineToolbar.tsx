// The pipeline-view toolbar (#46): add a pass, and (when a pass is selected)
// remove it, move it left/right, or drill into it. Pass order IS the .slangp
// pass index, so the move buttons reorder Project.passes (with index remap).
import { useDocumentStore } from "../store/documentStore";

export function PipelineToolbar() {
  const passCount = useDocumentStore((s) => s.project.passes.length);
  const selectedPassId = useDocumentStore((s) => s.selections.pipeline);
  const selectedIndex = useDocumentStore((s) =>
    s.selections.pipeline === null
      ? -1
      : s.project.passes.findIndex((p) => p.id === s.selections.pipeline),
  );

  const addPass = useDocumentStore((s) => s.addPass);
  const removePass = useDocumentStore((s) => s.removePass);
  const reorderPass = useDocumentStore((s) => s.reorderPass);
  const openPass = useDocumentStore((s) => s.openPass);

  const hasSelection = selectedPassId !== null && selectedIndex >= 0;
  const canMoveLeft = hasSelection && selectedIndex > 0;
  const canMoveRight = hasSelection && selectedIndex < passCount - 1;
  const canRemove = hasSelection && passCount > 1;

  return (
    <div className="editor__toolbar" role="toolbar" aria-label="Pipeline actions">
      <button type="button" onClick={() => addPass()} title="Append a pass">
        Add pass
      </button>
      <span className="editor__toolbar-sep" aria-hidden="true" />
      <button
        type="button"
        onClick={() => hasSelection && reorderPass(selectedIndex, selectedIndex - 1)}
        disabled={!canMoveLeft}
        title="Move pass earlier"
      >
        Move left
      </button>
      <button
        type="button"
        onClick={() => hasSelection && reorderPass(selectedIndex, selectedIndex + 1)}
        disabled={!canMoveRight}
        title="Move pass later"
      >
        Move right
      </button>
      <button
        type="button"
        onClick={() => selectedPassId && removePass(selectedPassId)}
        disabled={!canRemove}
        title="Remove pass"
      >
        Remove pass
      </button>
      <span className="editor__toolbar-sep" aria-hidden="true" />
      <button
        type="button"
        onClick={() => selectedPassId && openPass(selectedPassId)}
        disabled={!hasSelection}
        title="Open pass graph"
      >
        Open pass
      </button>
    </div>
  );
}
