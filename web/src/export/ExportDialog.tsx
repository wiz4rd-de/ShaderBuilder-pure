// The export-bundle dialog (#64): destination + bundle name + a summary of the
// passes/params/LUTs to be written, with "Export" DISABLED while the graph is
// invalid (the blocking reasons listed + linkable into the editor), and a
// post-export confirmation showing the written path + a "reveal" action. A write
// failure (e.g. permission denied) surfaces as a NON-FATAL error, not a crash.
import { basename } from "../session/paths";
import { useDocumentStore } from "../store/documentStore";
import { blockingReasons, summarizeProject, successMessage } from "./exportGate";
import { useExportStore } from "./exportStore";

export function ExportDialog(): React.JSX.Element | null {
  const open = useExportStore((s) => s.open);
  const phase = useExportStore((s) => s.phase);
  const destDir = useExportStore((s) => s.destDir);
  const bundleName = useExportStore((s) => s.bundleName);
  const validation = useExportStore((s) => s.validation);
  const validating = useExportStore((s) => s.validating);
  const result = useExportStore((s) => s.result);
  const errorMessage = useExportStore((s) => s.errorMessage);
  const setBundleName = useExportStore((s) => s.setBundleName);
  const chooseDestination = useExportStore((s) => s.chooseDestination);
  const runExport = useExportStore((s) => s.runExport);
  const reveal = useExportStore((s) => s.reveal);
  const closeDialog = useExportStore((s) => s.closeDialog);

  const project = useDocumentStore((s) => s.project);
  const pipelineValid = useDocumentStore((s) => s.pipelineValid);
  const openPass = useDocumentStore((s) => s.openPass);
  const setSelection = useDocumentStore((s) => s.setSelection);

  if (!open) {
    return null;
  }

  const summary = summarizeProject(project);
  const reasons = blockingReasons(pipelineValid, validation?.blockers ?? []);
  const exportable = !validating && reasons.length === 0;
  const canExport =
    exportable && destDir !== null && bundleName.trim().length > 0 && phase === "form";

  // Jump to the offending pass in the editor (links into the Problems panel flow).
  const jumpTo = (passId: string | null): void => {
    if (!passId) {
      return;
    }
    openPass(passId);
    setSelection({ nodeIds: [], edgeIds: [] });
    closeDialog();
  };

  return (
    <div className="confirm__backdrop" role="presentation">
      <div
        className="confirm__dialog export-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="Export bundle"
      >
        <h2 className="export-dialog__title">Export RetroArch bundle</h2>

        {phase === "done" && result ? (
          <div className="export-dialog__done" data-testid="export-done">
            <p className="export-dialog__success">{successMessage(result)}</p>
            {result.warnings.length > 0 ? (
              <ul className="export-dialog__warnings" aria-label="Export warnings">
                {result.warnings.map((w, i) => (
                  <li key={i} className="export-dialog__warning">
                    {w}
                  </li>
                ))}
              </ul>
            ) : null}
            {errorMessage ? (
              <p className="export-dialog__error" role="alert">
                {errorMessage}
              </p>
            ) : null}
            <div className="confirm__actions">
              <button
                type="button"
                className="confirm__btn confirm__btn--primary"
                onClick={() => void reveal()}
              >
                Reveal in file manager
              </button>
              <button type="button" className="confirm__btn" onClick={closeDialog}>
                Close
              </button>
            </div>
          </div>
        ) : (
          <div className="export-dialog__form">
            {/* Destination directory. */}
            <label className="export-dialog__field">
              <span className="export-dialog__label">Destination folder</span>
              <div className="export-dialog__row">
                <span className="export-dialog__dest" data-testid="export-dest">
                  {destDir ?? "No folder chosen"}
                </span>
                <button
                  type="button"
                  className="confirm__btn"
                  onClick={() => void chooseDestination()}
                  disabled={phase === "exporting"}
                >
                  Choose…
                </button>
              </div>
            </label>

            {/* Bundle name. */}
            <label className="export-dialog__field">
              <span className="export-dialog__label">Bundle name</span>
              <input
                type="text"
                className="export-dialog__input"
                value={bundleName}
                onChange={(e) => setBundleName(e.target.value)}
                disabled={phase === "exporting"}
                aria-label="Bundle name"
              />
            </label>

            {/* Summary of what would be written. */}
            <dl className="export-dialog__summary" data-testid="export-summary">
              <div>
                <dt>Passes</dt>
                <dd>{summary.passCount}</dd>
              </div>
              <div>
                <dt>Parameters</dt>
                <dd>{summary.parameterCount}</dd>
              </div>
              <div>
                <dt>LUTs</dt>
                <dd>{summary.lutCount}</dd>
              </div>
            </dl>

            {/* Blocking reasons (Export disabled while present). */}
            {reasons.length > 0 ? (
              <div className="export-dialog__blockers" data-testid="export-blockers">
                <p className="export-dialog__blockers-title">
                  {validating ? "Checking the project…" : "Cannot export yet:"}
                </p>
                {!validating ? (
                  <ul>
                    {reasons.map((r, i) => (
                      <li key={i} className="export-dialog__blocker">
                        <span>{r.message}</span>
                        {r.passId ? (
                          <button
                            type="button"
                            className="export-dialog__jump"
                            onClick={() => jumpTo(r.passId)}
                          >
                            Go to pass
                          </button>
                        ) : null}
                      </li>
                    ))}
                  </ul>
                ) : null}
              </div>
            ) : null}

            {/* A non-fatal export failure. */}
            {phase === "error" && errorMessage ? (
              <p className="export-dialog__error" role="alert" data-testid="export-error">
                {errorMessage}
              </p>
            ) : null}

            <div className="confirm__actions">
              <button
                type="button"
                className="confirm__btn confirm__btn--primary"
                onClick={() => void runExport()}
                disabled={!canExport}
                data-testid="export-confirm"
              >
                {phase === "exporting" ? "Exporting…" : "Export"}
                {destDir ? ` to ${basename(destDir)}/${bundleName.trim()}` : ""}
              </button>
              <button
                type="button"
                className="confirm__btn"
                onClick={closeDialog}
                disabled={phase === "exporting"}
              >
                Cancel
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
