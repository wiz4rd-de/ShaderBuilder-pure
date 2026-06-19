// The PROBLEMS panel (#54) — the aggregate list of every compile diagnostic across
// the pipeline, tagged with the pass it came from. Reads the store's `problems`
// list (populated by the live compile loop). Clicking a problem drills into its
// pass and selects the offending node so the editor jumps to it.
//
// This is the project-wide counterpart to the per-node inspector diagnostics (#47):
// the inspector shows the SELECTED node's problems; this shows ALL of them.
import { useDocumentStore } from "../store/documentStore";

export function ProblemsPanel(): React.JSX.Element {
  const compileProblems = useDocumentStore((s) => s.problems);
  const engineProblems = useDocumentStore((s) => s.engineProblems);
  const pipelineValid = useDocumentStore((s) => s.pipelineValid);
  const compiling = useDocumentStore((s) => s.compiling);
  const openPass = useDocumentStore((s) => s.openPass);
  const setSelection = useDocumentStore((s) => s.setSelection);

  // Show the compile-loop diagnostics AND the engine-synthesized render/compile
  // errors (#62) in one list — the engine ones (a whole-pass slang failure, a
  // device-lost) are tagged `origin: "engine"` and appended after the compile set.
  const problems = [...compileProblems, ...engineProblems];

  const errorCount = problems.filter((p) => p.diagnostic.severity === "error").length;
  const warningCount = problems.length - errorCount;

  const jumpTo = (passId: string, nodeId: string): void => {
    if (passId === "") {
      return; // a pipeline-wide engine error with no pass to navigate to
    }
    openPass(passId);
    // Only select a node when the diagnostic names one (engine errors may not).
    if (nodeId !== "") {
      setSelection({ nodeIds: [nodeId], edgeIds: [] });
    }
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
          {problems.map((p, i) => {
            const navigable = p.passId !== "";
            return (
              <li
                key={`${p.origin ?? "compile"}-${p.passId}-${i}`}
                className={`problems__item problems__item--${p.diagnostic.severity}`}
              >
                <button
                  type="button"
                  className="problems__jump"
                  disabled={!navigable}
                  onClick={() => jumpTo(p.passId, p.diagnostic.node)}
                  title={
                    navigable
                      ? `Go to ${p.diagnostic.node ? `node ${p.diagnostic.node} in ` : ""}${p.passName}`
                      : p.passName
                  }
                >
                  <span className="problems__pass">{p.passName}</span>
                  {p.origin === "engine" ? (
                    <span className="problems__origin">engine</span>
                  ) : null}
                  <span className="problems__code">{p.diagnostic.code}</span>
                  <span className="problems__message">{p.diagnostic.message}</span>
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}
