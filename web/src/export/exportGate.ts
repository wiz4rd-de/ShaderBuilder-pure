// Pure export-gate logic (#64): turn the document + the Rust validation result
// into the dialog's view-model — the pass/param/LUT summary, the blocking reasons
// (each linkable into the Problems panel), and the post-export outcome message.
// Pure (no Tauri / no React) so it unit-tests directly.
import type { ExportBlocker } from "../bindings/ExportBlocker";
import type { ExportError } from "../bindings/ExportError";
import type { ExportResult } from "../bindings/ExportResult";
import type { Project } from "../bindings/Project";
import type { ExportProjectOutcome } from "../compile/exportProject";

/** A one-line, user-facing summary of what the bundle would contain. */
export interface ExportSummary {
  passCount: number;
  parameterCount: number;
  lutCount: number;
}

/** Count the passes / parameters / LUTs the bundle would write. */
export function summarizeProject(project: Project): ExportSummary {
  return {
    passCount: project.passes.length,
    parameterCount: project.parameters.length,
    lutCount: project.luts.length,
  };
}

/** One blocking reason, rendered for the dialog with an optional pass to jump to. */
export interface BlockingReason {
  /** The human-readable reason text. */
  message: string;
  /** The owning pass id, when the reason names one (links into the Problems panel). */
  passId: string | null;
}

/** Render a structured {@link ExportBlocker} as a {@link BlockingReason}. */
export function blockerToReason(blocker: ExportBlocker): BlockingReason {
  switch (blocker.kind) {
    case "noPasses":
      return { message: "The project has no passes to export.", passId: null };
    case "uncompiledGraphPass":
      return {
        message: `Pass "${blocker.passName}" is an unresolved node graph — it did not compile, so it has no shader to export.`,
        passId: blocker.passId,
      };
    case "emptyPassSource":
      return {
        message: `Pass "${blocker.passName}" has an empty source body.`,
        passId: blocker.passId,
      };
  }
}

/**
 * Combine the live `pipelineValid` flag with the Rust validation blockers into the
 * full blocking-reason list (#64). The two are complementary: `pipelineValid`
 * catches a graph pass that did not compile (the live loop's verdict) BEFORE the
 * substitution would even produce a project to validate, while the Rust gate
 * catches the structural cases (no passes, empty source, a still-graph pass).
 *
 * Returns an empty list ⇒ the project is exportable; a non-empty list ⇒ "Export"
 * is disabled and the reasons are shown.
 */
export function blockingReasons(
  pipelineValid: boolean | null,
  blockers: ExportBlocker[],
): BlockingReason[] {
  const reasons = blockers.map(blockerToReason);
  // The pipeline being invalid (a pass that does not compile) is a blocker on its
  // own even if the structural gate found nothing yet — surface it explicitly so
  // the user is told to fix the graph first. Avoid a duplicate when the gate
  // already reported an uncompiled graph pass.
  if (pipelineValid === false && !blockers.some((b) => b.kind === "uncompiledGraphPass")) {
    reasons.unshift({
      message:
        "The pipeline is not renderable — fix the blocking problems before exporting.",
      passId: null,
    });
  }
  if (pipelineValid === null && reasons.length === 0) {
    reasons.push({
      message: "The project has not compiled yet — wait for the live compile to finish.",
      passId: null,
    });
  }
  return reasons;
}

/** A user-facing message describing a non-`ok` export outcome (non-fatal). */
export function outcomeErrorMessage(
  outcome: Exclude<ExportProjectOutcome, { kind: "ok" }>,
): string {
  if (outcome.kind === "notRenderable") {
    const ids = outcome.uncompiledPassIds.join(", ");
    return `Export blocked: ${outcome.uncompiledPassIds.length} pass(es) did not compile (${ids}). No files were written.`;
  }
  return exportErrorMessage(outcome.error);
}

/** A user-facing message for a typed {@link ExportError} (a write failure). */
export function exportErrorMessage(error: ExportError): string {
  if (error.kind === "io") {
    return `Could not write the bundle: ${error.message}`;
  }
  // GraphPassUnsupported should never fire post-substitution + gate; if it does it
  // is an internal error (the brief: surface it as such).
  return `Internal export error: pass "${error.passId}" reached the writer as a node graph. This should not happen — please report it.`;
}

/** A success message naming the written bundle path + file counts. */
export function successMessage(result: ExportResult): string {
  const parts = [`${result.passFiles.length} pass file(s)`];
  if (result.textureFiles.length > 0) {
    parts.push(`${result.textureFiles.length} LUT(s)`);
  }
  return `Exported ${parts.join(", ")} to ${result.presetPath}`;
}
