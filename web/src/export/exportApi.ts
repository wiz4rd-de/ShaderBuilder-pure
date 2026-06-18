// Typed IPC + native-dialog wrappers for the export-bundle UX (#64). This is the
// ONLY place the export command names + the directory-picker / reveal seams live,
// so the export dialog + its tests depend on a typed surface, not raw `invoke` /
// plugin strings. Mirrors session/api.ts (the #63 precedent the brief reuses).
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

import type { ExportValidation } from "../bindings/ExportValidation";
import type { Project } from "../bindings/Project";

/**
 * Run the Rust pre-export validation gate (#64) — the fail-closed check that
 * returns the structured blockers preventing export. An exportable project
 * returns `{ blockers: [] }`.
 */
export function validateExport(project: Project): Promise<ExportValidation> {
  return invoke<ExportValidation>("validate_export", { project });
}

/**
 * Show the native DIRECTORY picker (the #63 dialog plugin) for the export
 * destination; resolves to the chosen directory path or `null` on cancel.
 */
export async function pickExportDir(): Promise<string | null> {
  const picked = await openDialog({
    multiple: false,
    directory: true,
    title: "Choose an export destination folder",
  });
  if (picked === null) {
    return null;
  }
  return Array.isArray(picked) ? (picked[0] ?? null) : picked;
}

/**
 * Reveal a written path in the OS file manager (the opener plugin's
 * `revealItemInDir`). Rejects if the plugin is unavailable — the caller degrades
 * to just showing the path.
 */
export function revealPath(path: string): Promise<void> {
  return revealItemInDir(path);
}
