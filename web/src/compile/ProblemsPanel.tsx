// The PROBLEMS panel (#54) — the aggregate list of every compile diagnostic across
// the pipeline, tagged with the pass it came from. Reads the store's `problems`
// list (populated by the live compile loop). Clicking a problem drills into its
// pass and selects the offending node so the editor jumps to it.
//
// This is the project-wide counterpart to the per-node inspector diagnostics (#47):
// the inspector shows the SELECTED node's problems; this shows ALL of them.
import { useDocumentStore } from "../store/documentStore";

export function ProblemsPanel(): React.JSX.Element {
  const problems = useDocumentStore((s) => s.problems);
  const pipelineValid = useDocumentStore((s) => s.pipelineValid);
  const compiling = useDocumentStore((s) => s.compiling);
  const openPass = useDocumentStore((s) => s.openPass);
  const setSelection = useDocumentStore((s) => s.setSelection);

  const errorCount = problems.filter((p) => p.diagnostic.severity === "error").length;
  const warningCount = problems.length - errorCount;

  const jumpTo = (passId: string, nodeId: string): void => {
    openPass(passId);
    setSelection({ nodeIds: [nodeId], edgeIds: [] });
  };

  return (
    <div className="panel__body problems" aria-label="Problems">
      <div className="problems__summary" data-testid="problems-summary">
        {pipelineValid === false ? (
          <span className="problems__status problems__status--invalid">
            Pipeline not renderable
          </span>
        ) : pipelineValid === true ? (
          <span className="problems__status problems__status--valid">Pipeline OK</span>
        ) : (
          <span className="problems__status">{compiling ? "Compiling…" : "Not compiled yet"}</span>
        )}
        <span className="problems__counts">
          {errorCount} {errorCount === 1 ? "error" : "errors"}, {warningCount}{" "}
          {warningCount === 1 ? "warning" : "warnings"}
        </span>
      </div>

      {problems.length === 0 ? (
        <div className="panel__placeholder">
          {pipelineValid === false
            ? "The pipeline has a blocking problem but no node-level diagnostic."
            : "No problems."}
        </div>
      ) : (
        <ul className="problems__list">
          {problems.map((p, i) => (
            <li
              key={`${p.passId}-${i}`}
              className={`problems__item problems__item--${p.diagnostic.severity}`}
            >
              <button
                type="button"
                className="problems__jump"
                onClick={() => jumpTo(p.passId, p.diagnostic.node)}
                title={`Go to node ${p.diagnostic.node} in ${p.passName}`}
              >
                <span className="problems__pass">{p.passName}</span>
                <span className="problems__code">{p.diagnostic.code}</span>
                <span className="problems__message">{p.diagnostic.message}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
