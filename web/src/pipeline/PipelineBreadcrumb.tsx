// The two-level navigation breadcrumb (#46). At the pipeline level it shows just
// "Pipeline"; drilled into a pass it shows "Pipeline / <pass name>" with a back
// control that returns to the pipeline (restoring its pan/zoom + selection).
import { useDocumentStore } from "../store/documentStore";

export function PipelineBreadcrumb() {
  const level = useDocumentStore((s) => s.level);
  const activePassId = useDocumentStore((s) => s.activePassId);
  const passName = useDocumentStore(
    (s) => s.project.passes.find((p) => p.id === s.activePassId)?.name ?? "",
  );
  const showPipeline = useDocumentStore((s) => s.showPipeline);

  return (
    <nav className="pipeline-breadcrumb" aria-label="Editor navigation">
      {level === "pass" ? (
        <button
          type="button"
          className="pipeline-breadcrumb__back"
          onClick={showPipeline}
          title="Back to pipeline"
          aria-label="Back to pipeline"
        >
          ‹ Pipeline
        </button>
      ) : (
        <span className="pipeline-breadcrumb__crumb pipeline-breadcrumb__crumb--current">
          Pipeline
        </span>
      )}
      {level === "pass" ? (
        <>
          <span className="pipeline-breadcrumb__sep" aria-hidden="true">
            /
          </span>
          <span
            className="pipeline-breadcrumb__crumb pipeline-breadcrumb__crumb--current"
            data-testid="breadcrumb-pass"
            data-pass-id={activePassId}
          >
            {passName}
          </span>
        </>
      ) : null}
    </nav>
  );
}
