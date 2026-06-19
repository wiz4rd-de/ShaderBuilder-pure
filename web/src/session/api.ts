// Typed IPC wrappers (#63) over the project save/load + session Tauri commands,
// plus the native file-dialog helpers. This is the ONLY place the project/session
// command names + dialog filters live, so the File-menu flow and its tests depend
// on a typed surface, not raw `invoke`/dialog strings.
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";

import type { Project } from "../bindings/Project";
import type { RecentProject } from "../bindings/RecentProject";
import type { Recovery } from "../bindings/Recovery";

/** The native file filter used for the project open/save dialogs. */
export const PROJECT_FILTER = {
  name: "ShaderBuilder Project",
  extensions: ["json"],
};

/** Save a project to a native `.json` file (Tauri `save_project`, #38/#63). */
export function saveProject(path: string, project: Project): Promise<void> {
  return invoke<void>("save_project", { path, project });
}

/** Load a project from a native `.json` file (Tauri `load_project`, #38/#63). */
export function loadProject(path: string): Promise<Project> {
  return invoke<Project>("load_project", { path });
}

/** List the recent projects, pruned of missing files (Tauri `load_recents`, #63). */
export function loadRecents(): Promise<RecentProject[]> {
  return invoke<RecentProject[]>("load_recents");
}

/** Push a freshly opened/saved project to the recents list (Tauri `push_recent`, #63). */
export function pushRecent(entry: RecentProject): Promise<RecentProject[]> {
  return invoke<RecentProject[]>("push_recent", { entry });
}

/** Mirror the JS dirty flag into the backend (Tauri `set_dirty`, #63). */
export function setBackendDirty(dirty: boolean): Promise<void> {
  return invoke<void>("set_dirty", { dirty });
}

/** Autosave the working document to the recovery file (Tauri `autosave_recovery`, #63). */
export function autosaveRecovery(
  project: Project,
  projectPath: string | null,
): Promise<void> {
  return invoke<void>("autosave_recovery", { project, projectPath });
}

/** Clear the recovery file after a save / a restore decision (Tauri `clear_recovery`, #63). */
export function clearRecovery(): Promise<void> {
  return invoke<void>("clear_recovery");
}

/** On launch, return a recovery offer if one is newer than the last save (#63). */
export function checkRecovery(): Promise<Recovery | null> {
  return invoke<Recovery | null>("check_recovery");
}

/**
 * Show the native OPEN dialog filtered to project files; resolves to the chosen
 * path or `null` if the user cancelled (#63).
 */
export async function pickOpenPath(): Promise<string | null> {
  const picked = await openDialog({
    multiple: false,
    directory: false,
    filters: [PROJECT_FILTER],
  });
  // The plugin returns a string for a single selection, an array for multiple
  // (we asked for single), or null on cancel.
  if (picked === null) {
    return null;
  }
  return Array.isArray(picked) ? (picked[0] ?? null) : picked;
}

/**
 * Show the native SAVE dialog filtered to project files; resolves to the chosen
 * path or `null` if cancelled (#63). `defaultName` seeds the filename.
 */
export async function pickSavePath(defaultName?: string): Promise<string | null> {
  return saveDialog({
    defaultPath: defaultName ? `${defaultName}.json` : undefined,
    filters: [PROJECT_FILTER],
  });
}
