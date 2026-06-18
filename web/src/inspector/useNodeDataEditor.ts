// The inspector's write path into the document store (#47). Two edit modes:
//
//   * live(patch)   — for per-keystroke text/number typing: opens a coalesced
//     interaction (beginInteraction) on the first keystroke, applies each patch
//     live WITHOUT history, and arms a debounce that commits ONE undo entry once
//     typing settles (or on blur/flush). So a burst of keystrokes = one undo.
//   * commit(patch) — for atomic edits (a select, a checkbox, a port add/remove):
//     flushes any open live edit, then writes the patch as ONE discrete entry.
//
// The debounce timer + the "interaction open?" flag are per-hook-instance refs so
// a field can fire `live` repeatedly without re-opening the interaction.
import { useCallback, useEffect, useRef } from "react";

import { useDocumentStore } from "../store/documentStore";

/** Milliseconds of idle after the last keystroke before a live edit commits. */
export const INSPECTOR_DEBOUNCE_MS = 350;

export interface NodeDataEditor {
  /** Per-keystroke edit: live-apply now, commit one coalesced entry when idle. */
  live: (patch: Record<string, unknown>) => void;
  /** Atomic edit: flush any open live edit, then commit this patch as one entry. */
  commit: (patch: Record<string, unknown>) => void;
  /** Force any pending live edit to commit now (e.g. on blur / unmount). */
  flush: () => void;
}

/**
 * A stable editor bound to one node id. The returned callbacks are stable across
 * renders (so fields don't churn), reading the live store actions on each call.
 */
export function useNodeDataEditor(nodeId: string | null): NodeDataEditor {
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const interactionOpen = useRef(false);

  const closeInteraction = useCallback(() => {
    if (timer.current !== null) {
      clearTimeout(timer.current);
      timer.current = null;
    }
    if (interactionOpen.current) {
      interactionOpen.current = false;
      useDocumentStore.getState().commit();
    }
  }, []);

  const flush = useCallback(() => closeInteraction(), [closeInteraction]);

  const live = useCallback(
    (patch: Record<string, unknown>) => {
      if (!nodeId) {
        return;
      }
      if (!interactionOpen.current) {
        interactionOpen.current = true;
        useDocumentStore.getState().beginInteraction();
      }
      useDocumentStore.getState().patchNodeData(nodeId, patch);
      if (timer.current !== null) {
        clearTimeout(timer.current);
      }
      timer.current = setTimeout(closeInteraction, INSPECTOR_DEBOUNCE_MS);
    },
    [nodeId, closeInteraction],
  );

  const commit = useCallback(
    (patch: Record<string, unknown>) => {
      if (!nodeId) {
        return;
      }
      // Settle any open coalesced edit first so its undo entry lands before this
      // atomic one (rather than swallowing this patch into the same entry).
      closeInteraction();
      useDocumentStore.getState().updateNodeData(nodeId, patch);
    },
    [nodeId, closeInteraction],
  );

  // Commit any in-flight live edit if the inspector unmounts (or the node id
  // changes), so a half-typed value is never stranded as a non-undoable edit.
  useEffect(() => closeInteraction, [nodeId, closeInteraction]);

  return { live, commit, flush };
}
