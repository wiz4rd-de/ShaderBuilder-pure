// The navigation breadcrumb (#46, extended #57). At the pipeline level it shows
// just "Pipeline"; drilled into a pass it shows "Pipeline / <pass name>"; drilled
// further into subgraph interiors it appends "/ <subgraph name>" per level. The
// back control returns one level (pop a subgraph level, or back to the pipeline),
// restoring that level's pan/zoom + selection.
import { resolveGraph, subgraphAt } from "../store/subgraphNav";
import { useDocumentStore } from "../store/documentStore";

export function PipelineBreadcrumb() {
  const level = useDocumentStore((s) => s.level);
  const activePassId = useDocumentStore((s) => s.activePassId);
  const subgraphPath = useDocumentStore((s) => s.subgraphPath);
  const passName = useDocumentStore(
    (s) => s.project.passes.find((p) => p.id === s.activePassId)?.name ?? "",
  );
  // The display name of each subgraph node along the drill-in path.
  const subgraphNames = useDocumentStore((s) => {
    const names: string[] = [];
    for (let i = 0; i < s.subgraphPath.length; i += 1) {
      const prefix = s.subgraphPath.slice(0, i);
      const graph = resolveGraph(s.project, s.activePassId, prefix);
      const sub = subgraphAt(graph, s.subgraphPath[i]!);
      names.push(sub?.name ?? "Subgraph");
    }
    return names;
  });
  const showPipeline = useDocumentStore((s) => s.showPipeline);
  const closeSubgraph = useDocumentStore((s) => s.closeSubgraph);

  // The back control pops ONE level: a subgraph level if drilled in, else the
  // pipeline. At the pass level the label/aria stay exactly "Pipeline" (the
  // #46 contract); inside a subgraph it names the parent (the enclosing
  // subgraph, or the pass at depth 1).
  const onBack = subgraphPath.length > 0 ? closeSubgraph : showPipeline;
  const backLabel =
    subgraphPath.length === 0
      ? "Pipeline"
      : subgraphNames.length > 1
        ? subgraphNames[subgraphNames.length - 2]!
        : passName;
  const backAria = subgraphPath.length === 0 ? "Back to pipeline" : `Back to ${backLabel}`;

  return (
    <nav className="pipeline-breadcrumb" aria-label="Editor navigation">
      {level === "pass" ? (
        <button
          type="button"
          className="pipeline-breadcrumb__back"
          onClick={onBack}
          title={backAria}
          aria-label={backAria}
        >
          ‹ {backLabel}
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
            className={
              "pipeline-breadcrumb__crumb" +
              (subgraphPath.length === 0 ? " pipeline-breadcrumb__crumb--current" : "")
            }
            data-testid="breadcrumb-pass"
            data-pass-id={activePassId}
          >
            {passName}
          </span>
          {subgraphNames.map((name, i) => (
            <span key={subgraphPath[i]} className="pipeline-breadcrumb__crumb-group">
              <span className="pipeline-breadcrumb__sep" aria-hidden="true">
                /
              </span>
              <span
                className={
                  "pipeline-breadcrumb__crumb" +
                  (i === subgraphNames.length - 1
                    ? " pipeline-breadcrumb__crumb--current"
                    : "")
                }
                data-testid="breadcrumb-subgraph"
                data-subgraph-id={subgraphPath[i]}
              >
                {name}
              </span>
            </span>
          ))}
        </>
      ) : null}
    </nav>
  );
}
