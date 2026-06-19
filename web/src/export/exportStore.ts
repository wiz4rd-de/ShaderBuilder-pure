// The export-dialog state machine (#64). Owns the open/close lifecycle, the
// destination + bundle-name fields, the live validation result, and the outcome
// (a written-bundle confirmation or a non-fatal failure). Kept OUT of the document
// store (pure UI ephemera, never part of an undo snapshot). All Tauri/IO seams are
// injected so the flow unit-tests with no runtime.
import { create } from "zustand";

import type { ExportResult } from "../bindings/ExportResult";
import type { ExportValidation } from "../bindings/ExportValidation";
import type { Project } from "../bindings/Project";
import {
  exportProject as defaultExportProject,
  type ExportDeps,
} from "../compile/exportProject";
import { useDocumentStore } from "../store/documentStore";
import {
  pickExportDir as defaultPickExportDir,
  revealPath as defaultRevealPath,
  validateExport as defaultValidateExport,
} from "./exportApi";
import { outcomeErrorMessage, successMessage } from "./exportGate";

/** Which screen the dialog is showing. */
export type ExportPhase =
  | "form" // choosing destination + name; Export gated on validity
  | "exporting" // the writer is running
  | "done" // success — show the path + reveal action
  | "error"; // a non-fatal failure (e.g. permission denied)

/** Injectable IO seams (defaults are the real wrappers) so the flow is testable. */
export interface ExportFlowDeps {
  validateExport: (project: Project) => Promise<ExportValidation>;
  pickExportDir: () => Promise<string | null>;
  revealPath: (path: string) => Promise<void>;
  /** Compile + substitute + write the bundle (the existing exportProject flow). */
  exportProject: typeof defaultExportProject;
  /** Extra deps threaded into exportProject (the injected compile/export callers). */
  exportProjectDeps?: ExportDeps;
}

export const defaultExportFlowDeps: ExportFlowDeps = {
  validateExport: defaultValidateExport,
  pickExportDir: defaultPickExportDir,
  revealPath: defaultRevealPath,
  exportProject: defaultExportProject,
};

interface ExportState {
  open: boolean;
  phase: ExportPhase;
  /** The chosen destination directory, or `null` until picked. */
  destDir: string | null;
  /** The bundle folder name the user can edit (seeded from the project name). */
  bundleName: string;
  /** The latest validation result (drives the disabled state + reasons). */
  validation: ExportValidation | null;
  /** Whether validation is in flight. */
  validating: boolean;
  /** The successful export report (on `done`). */
  result: ExportResult | null;
  /** A non-fatal failure message (on `error`), or a reveal failure note. */
  errorMessage: string | null;

  /** Open the dialog: seed the bundle name + run the validation gate. */
  openDialog: (deps?: ExportFlowDeps) => Promise<void>;
  /** Close the dialog and reset its transient state. */
  closeDialog: () => void;
  setBundleName: (name: string) => void;
  /** Pick the destination directory via the native picker. */
  chooseDestination: (deps?: ExportFlowDeps) => Promise<void>;
  /** Run the export (compile + substitute + write); route the outcome. */
  runExport: (deps?: ExportFlowDeps) => Promise<void>;
  /** Reveal the written bundle in the OS file manager; degrade to a note on failure. */
  reveal: (deps?: ExportFlowDeps) => Promise<void>;
}

/** Derive a safe default bundle folder name from the project name. */
export function defaultBundleName(projectName: string): string {
  const cleaned = projectName
    .trim()
    .replace(/[^A-Za-z0-9._-]+/g, "_")
    .replace(/^_+|_+$/g, "");
  return cleaned.length > 0 ? cleaned : "preset";
}

/** Join a directory and a bundle name into the full destination path. */
export function joinDest(destDir: string, bundleName: string): string {
  const sep = destDir.includes("\\") ? "\\" : "/";
  const trimmed = destDir.endsWith(sep) ? destDir.slice(0, -1) : destDir;
  return `${trimmed}${sep}${bundleName}`;
}

export const useExportStore = create<ExportState>((set, get) => ({
  open: false,
  phase: "form",
  destDir: null,
  bundleName: "preset",
  validation: null,
  validating: false,
  result: null,
  errorMessage: null,

  openDialog: async (deps = defaultExportFlowDeps) => {
    const project = useDocumentStore.getState().project;
    set({
      open: true,
      phase: "form",
      destDir: null,
      bundleName: defaultBundleName(project.name),
      validation: null,
      validating: true,
      result: null,
      errorMessage: null,
    });
    try {
      const validation = await deps.validateExport(project);
      // Ignore a stale result if the dialog was closed meanwhile.
      if (get().open) {
        set({ validation, validating: false });
      }
    } catch {
      if (get().open) {
        set({ validation: { blockers: [] }, validating: false });
      }
    }
  },

  closeDialog: () =>
    set({
      open: false,
      phase: "form",
      destDir: null,
      validation: null,
      validating: false,
      result: null,
      errorMessage: null,
    }),

  setBundleName: (name) => set({ bundleName: name }),

  chooseDestination: async (deps = defaultExportFlowDeps) => {
    const dir = await deps.pickExportDir();
    if (dir !== null) {
      set({ destDir: dir });
    }
  },

  runExport: async (deps = defaultExportFlowDeps) => {
    const { destDir, bundleName } = get();
    if (destDir === null || bundleName.trim().length === 0) {
      return;
    }
    const project = useDocumentStore.getState().project;
    set({ phase: "exporting", errorMessage: null });
    const fullDest = joinDest(destDir, bundleName.trim());
    const outcome = await deps.exportProject(project, fullDest, deps.exportProjectDeps);
    if (outcome.kind === "ok") {
      set({ phase: "done", result: outcome.result });
    } else {
      set({ phase: "error", errorMessage: outcomeErrorMessage(outcome) });
    }
  },

  reveal: async (deps = defaultExportFlowDeps) => {
    const result = get().result;
    if (!result) {
      return;
    }
    try {
      await deps.revealPath(result.presetPath);
    } catch {
      // The opener plugin may be unavailable — degrade to showing the path (it is
      // already shown) and note that reveal is not available.
      set({
        errorMessage: `Could not open the file manager — the bundle is at ${result.presetPath}`,
      });
    }
  },
}));

/** Re-export the success message helper for the dialog. */
export { successMessage };
