// Typed IPC wrappers (#59) over the three Phase-6 library-store Tauri commands
// (#58): list / save / delete. These are thin `invoke` shims — the only place
// the library command NAMES and argument shapes live — so the panel + tests
// depend on a typed surface, not raw `invoke` strings. The commands persist to
// an on-disk library dir (cross-project: an item saved here is visible from any
// project), so the panel just lists/saves/deletes and refreshes.
import { invoke } from "@tauri-apps/api/core";

import type { LibraryItem } from "../bindings/LibraryItem";

/** List every persisted library item (Tauri `list_library_node`, #58). */
export function listLibraryNode(): Promise<LibraryItem[]> {
  return invoke<LibraryItem[]>("list_library_node");
}

/** Persist a library item (Tauri `save_library_node`, #58). */
export function saveLibraryNode(item: LibraryItem): Promise<void> {
  return invoke<void>("save_library_node", { item });
}

/** Delete a library item by id (Tauri `delete_library_node`, #58). */
export function deleteLibraryNode(id: string): Promise<void> {
  return invoke<void>("delete_library_node", { id });
}
