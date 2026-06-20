// The File-menu flows (#63): New / Open / Open-Recent / Save / Save-As, plus the
// confirm-discard guard shared by New/Open/window-close. These orchestrate the
// document store, the typed session API, the confirm modal, and toasts — kept as
// plain async functions (not a hook) so they are callable from menu items,
// keyboard shortcuts, the recovery prompt, AND the window-close handler, and so
// they unit-test by injecting fakes for the IO/dialog seams.
import { basename } from "./paths";
import { useToastStore } from "../feedback/toastStore";
import { makeProject } from "../store/factories";
import { useConfirmStore } from "./confirmStore";
import { useDocumentStore } from "../store/documentStore";
import {
  autosaveRecovery,
  clearRecovery,
  loadProject as ipcLoadProject,
  loadRecents,
  pickOpenPath,
  pickSavePath,
  pushRecent,
  saveProject as ipcSaveProject,
} from "./api";
import type { Project } from "../bindings/Project";
import type { RecentProject } from "../bindings/RecentProject";

/**
 * The IO/dialog seams the flows depend on, injected so the tests can run with no
 * Tauri runtime. The defaults are the real `./api` wrappers.
 */
export interface SessionDeps {
  saveProject: (path: string, project: Project) => Promise<void>;
  loadProject: (path: string) => Promise<Project>;
  pickOpenPath: () => Promise<string | null>;
  pickSavePath: (defaultName?: string) => Promise<string | null>;
  pushRecent: (entry: RecentProject) => Promise<RecentProject[]>;
  loadRecents: () => Promise<RecentProject[]>;
  clearRecovery: () => Promise<void>;
  autosaveRecovery: (project: Project, projectPath: string | null) => Promise<void>;
  /** The current time as an RFC3339 string, for stamping `metadata.modifiedAt`. */
  now: () => string;
}

export const defaultDeps: SessionDeps = {
  saveProject: ipcSaveProject,
  loadProject: ipcLoadProject,
  pickOpenPath,
  pickSavePath,
  pushRecent,
  loadRecents,
  clearRecovery,
  autosaveRecovery,
  now: () => new Date().toISOString(),
};

/**
 * Guard a destructive action (New / Open / close) behind the unsaved-changes
 * prompt. Returns `true` if it is safe to proceed: not dirty, or the user chose
 * "discard", or chose "save" AND the save succeeded. Returns `false` to ABORT (the
 * user cancelled, or the save they asked for failed). Saving routes through
 * `save()` so a never-saved doc gets a Save-As path first.
 */
export async function guardUnsaved(deps: SessionDeps = defaultDeps): Promise<boolean> {
  const store = useDocumentStore.getState();
  if (!store.dirty) {
    return true;
  }
  const choice = await useConfirmStore
    .getState()
    .ask(`Save changes to "${store.project.name}" before continuing?`, {
      confirm: "Save",
      discard: "Discard",
      cancel: "Cancel",
    });
  if (choice === "cancel") {
    return false;
  }
  if (choice === "discard") {
    return true;
  }
  // "confirm" (Save): only proceed if the save actually lands (Save-As may be
  // cancelled).
  return save(deps);
}

/** New: guard unsaved edits, then reset to a fresh untitled single-pass project. */
export async function newProject(deps: SessionDeps = defaultDeps): Promise<void> {
  if (!(await guardUnsaved(deps))) {
    return;
  }
  useDocumentStore.getState().reset();
  await deps.clearRecovery().catch(() => undefined);
}

/** Open: guard unsaved edits, pick a file, load it. */
export async function openProject(deps: SessionDeps = defaultDeps): Promise<void> {
  if (!(await guardUnsaved(deps))) {
    return;
  }
  const path = await deps.pickOpenPath();
  if (path === null) {
    return; // cancelled
  }
  await openPath(path, deps);
}

/**
 * Open a SPECIFIC path (used by Open-Recent). Guards unsaved edits first, then
 * loads — handling a missing/malformed file gracefully (toast + drop nothing; the
 * recents prune on the next `load_recents`).
 */
export async function openRecent(
  path: string,
  deps: SessionDeps = defaultDeps,
): Promise<void> {
  if (!(await guardUnsaved(deps))) {
    return;
  }
  await openPath(path, deps);
}

/** Load `path` into the store, record it in recents, clear any recovery. */
async function openPath(path: string, deps: SessionDeps): Promise<void> {
  try {
    const project = await deps.loadProject(path);
    useDocumentStore.getState().loadProject(project, undefined, path);
    await recordRecent(path, project.name, deps);
    await deps.clearRecovery().catch(() => undefined);
  } catch (err) {
    useToastStore.getState().push("error", `Could not open ${basename(path)}: ${describe(err)}`);
  }
}

/**
 * Save: write to the current path if there is one, else fall through to Save-As.
 * Returns `true` on a successful write, `false` if cancelled or it failed (so the
 * close/New/Open guard can abort).
 */
export async function save(deps: SessionDeps = defaultDeps): Promise<boolean> {
  const path = useDocumentStore.getState().currentProjectPath;
  if (path === null) {
    return saveAs(deps);
  }
  return writeTo(path, deps);
}

/** Save-As: always prompt for a path, then write. Returns success like `save()`. */
export async function saveAs(deps: SessionDeps = defaultDeps): Promise<boolean> {
  const { project } = useDocumentStore.getState();
  const path = await deps.pickSavePath(project.name);
  if (path === null) {
    return false; // cancelled
  }
  return writeTo(path, deps);
}

/**
 * Stamp `modifiedAt`, write the document to `path`, mark it saved, record the
 * recent, and clear the recovery file. Toasts + returns `false` on an IO error.
 */
async function writeTo(path: string, deps: SessionDeps): Promise<boolean> {
  const store = useDocumentStore.getState();
  const now = deps.now();
  const project: Project = {
    ...store.project,
    metadata: {
      ...store.project.metadata,
      createdAt: store.project.metadata.createdAt ?? now,
      modifiedAt: now,
    },
  };
  try {
    await deps.saveProject(path, project);
  } catch (err) {
    useToastStore.getState().push("error", `Could not save ${basename(path)}: ${describe(err)}`);
    return false;
  }
  // Commit the stamped metadata + saved path into the store. The `project` snapshot
  // above was captured BEFORE the async write, so re-reading the CURRENT store
  // project here preserves any edits made during the save round-trip (#63 / F10)
  // rather than clobbering them with the stale pre-await copy. We restamp only the
  // createdAt/modifiedAt metadata onto whatever the live document is now, then let
  // `markSaved` clear dirty + record the path.
  const live = useDocumentStore.getState().project;
  useDocumentStore.setState({
    project: {
      ...live,
      metadata: {
        ...live.metadata,
        createdAt: live.metadata.createdAt ?? now,
        modifiedAt: now,
      },
    },
  });
  useDocumentStore.getState().markSaved(path);
  await recordRecent(path, project.name, deps);
  await deps.clearRecovery().catch(() => undefined);
  return true;
}

/** Add `path` to the recents list, swallowing a recents-store hiccup (non-fatal). */
async function recordRecent(
  path: string,
  name: string,
  deps: SessionDeps,
): Promise<void> {
  await deps.pushRecent({ path, name }).catch(() => undefined);
}

/** A short string for an unknown thrown value (typed error payload or Error). */
function describe(err: unknown): string {
  if (err && typeof err === "object" && "message" in err) {
    return String((err as { message: unknown }).message);
  }
  return String(err);
}

/**
 * Replace the live document with a recovered autosave (#63 launch restore). Loads
 * it as an untitled-or-pathed document, leaves it DIRTY (it has unsaved work), and
 * clears the recovery file so we do not re-offer it next launch.
 */
export async function restoreRecovery(
  project: Project,
  projectPath: string | null,
  deps: SessionDeps = defaultDeps,
): Promise<void> {
  const store = useDocumentStore.getState();
  store.loadProject(project, undefined, projectPath);
  // A recovered doc is unsaved by definition — re-flag it dirty after the load.
  useDocumentStore.setState({ dirty: true });
  await deps.clearRecovery().catch(() => undefined);
}

// Re-export the factory so callers that need a fresh project shape can reach it.
export { makeProject };
