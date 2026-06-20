// The start-screen actions (#66): the four ways out of the first-run screen —
// New, Open, Import preset, Open example. Each lands a document in the store and
// flips the onboarding `started` flag so the editor takes over. Kept as plain
// async functions (not a hook) so the start screen's buttons, and the unit tests
// (which inject fakes for the IO/dialog seams), share one code path.
//
// New / Open reuse the session `projectActions` (the same flows the File menu
// drives), so behaviour is identical whether reached from the start screen or the
// in-app menu. Because nothing is loaded yet on the start screen, `newProject` /
// `openProject` find a clean (non-dirty) doc and never prompt — the unsaved guard
// is a no-op here, which is correct.
import { useToastStore } from "../feedback/toastStore";
import { useDocumentStore } from "../store/documentStore";
import { useOnboardingStore } from "./onboardingStore";
import {
  newProject as sessionNew,
  openProject as sessionOpen,
} from "../session/projectActions";
import {
  importPreset as ipcImportPreset,
  loadExampleProject as ipcLoadExample,
  pickPresetPath as ipcPickPreset,
} from "./api";
import type { Project } from "../bindings/Project";

/** The IO/dialog seams the start actions depend on, injected for tests. */
export interface StartDeps {
  newProject: () => Promise<void>;
  openProject: () => Promise<void>;
  loadExampleProject: () => Promise<Project>;
  importPreset: (path: string) => Promise<Project>;
  pickPresetPath: () => Promise<string | null>;
}

export const defaultStartDeps: StartDeps = {
  newProject: sessionNew,
  openProject: sessionOpen,
  loadExampleProject: ipcLoadExample,
  importPreset: ipcImportPreset,
  pickPresetPath: ipcPickPreset,
};

/** Whether the document store currently holds a real (loaded) project. */
function markStarted(): void {
  useOnboardingStore.getState().markStarted();
}

/** Start screen → New: fresh untitled project, then enter the editor. */
export async function startNew(deps: StartDeps = defaultStartDeps): Promise<void> {
  await deps.newProject();
  markStarted();
}

/**
 * Start screen → Open: pick + load a `.json` project. Only enters the editor if a
 * project actually loaded (cancelling the picker leaves the start screen up).
 */
export async function startOpen(deps: StartDeps = defaultStartDeps): Promise<void> {
  const before = useDocumentStore.getState().currentProjectPath;
  await deps.openProject();
  // `openProject` sets `currentProjectPath` on a successful load; if it is still
  // unchanged the user cancelled the picker — stay on the start screen.
  if (useDocumentStore.getState().currentProjectPath !== before) {
    markStarted();
  }
}

/** Start screen → Open example: load the bundled example, then enter the editor. */
export async function startExample(deps: StartDeps = defaultStartDeps): Promise<void> {
  try {
    const project = await deps.loadExampleProject();
    // Load as an UNTITLED document (no path): the example is a starting point the
    // user saves under their own name, not a file to overwrite.
    useDocumentStore.getState().loadProject(project, undefined, null);
    markStarted();
  } catch (err) {
    useToastStore.getState().push("error", `Could not open the example: ${describe(err)}`);
  }
}

/**
 * Start screen → Import preset: pick a `.slangp`, import it to a project, enter
 * the editor. Stays on the start screen if the picker is cancelled or import
 * fails (toasted).
 */
export async function startImport(deps: StartDeps = defaultStartDeps): Promise<void> {
  const path = await deps.pickPresetPath();
  if (path === null) {
    return; // cancelled
  }
  try {
    const project = await deps.importPreset(path);
    useDocumentStore.getState().loadProject(project, undefined, null);
    markStarted();
  } catch (err) {
    useToastStore.getState().push("error", `Could not import preset: ${describe(err)}`);
  }
}

/** A short string for an unknown thrown value (typed error payload or Error). */
function describe(err: unknown): string {
  if (err && typeof err === "object" && "message" in err) {
    return String((err as { message: unknown }).message);
  }
  return String(err);
}
