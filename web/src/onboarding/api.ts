// Typed IPC wrappers for the onboarding start-screen actions (#66): load the
// bundled example project, and import an external `.slangp` preset (with its
// native open-dialog). Kept here — separate from the session API — so the
// start-screen flow depends on a typed surface, not raw `invoke`/dialog strings,
// and so the flow unit-tests by injecting fakes for these seams.
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import type { Project } from "../bindings/Project";

/** The native file filter for the import-preset open dialog. */
export const PRESET_FILTER = {
  name: "RetroArch Slang Preset",
  extensions: ["slangp"],
};

/** Load the bundled "CRT Scanlines + Curvature" example project (Tauri `load_example_project`). */
export function loadExampleProject(): Promise<Project> {
  return invoke<Project>("load_example_project");
}

/** Import a `.slangp` preset at `path` into an editor project (Tauri `import_preset`). */
export function importPreset(path: string): Promise<Project> {
  return invoke<Project>("import_preset", { path });
}

/**
 * Show the native OPEN dialog filtered to `.slangp` presets; resolves to the
 * chosen path or `null` if the user cancelled.
 */
export async function pickPresetPath(): Promise<string | null> {
  const picked = await openDialog({
    multiple: false,
    directory: false,
    filters: [PRESET_FILTER],
  });
  if (picked === null) {
    return null;
  }
  return Array.isArray(picked) ? (picked[0] ?? null) : picked;
}
