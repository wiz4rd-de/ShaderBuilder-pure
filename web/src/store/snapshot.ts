// Undo/redo is SNAPSHOT-based: each history entry is a deep, structural clone of
// the whole document plus the active-pass id, so undo restores the EXACT prior
// state (node ids, positions, edges, which pass was active) with no diffing.
// Selection and the clipboard are deliberately NOT part of a snapshot — undo
// moves the document, not the cursor.
import type { Project } from "../bindings/Project";

/** A point-in-time document state pushed onto the undo/redo stacks. */
export interface DocSnapshot {
  project: Project;
  activePassId: string;
}

/**
 * Deep-clone a value with no shared references. `structuredClone` is available
 * in the Tauri webview and in jsdom (Node ≥17); it round-trips the plain-JSON
 * document shape losslessly, which is exactly the schema invariant #45 needs.
 */
export function deepClone<T>(value: T): T {
  return structuredClone(value);
}

/** Clone a document snapshot so callers can't mutate a stored history entry. */
export function cloneSnapshot(snap: DocSnapshot): DocSnapshot {
  return { project: deepClone(snap.project), activePassId: snap.activePassId };
}
