// Drive the document store's selection from React Flow's CONTROLLED change
// channel (onNodesChange / onEdgesChange) rather than from `onSelectionChange`.
//
// WHY: the per-pass editor is a controlled React Flow canvas. React Flow emits a
// `select` change the moment you click; in controlled mode it expects you to apply
// it and feed the result back through the `nodes`/`edges` props. We previously
// dropped `select` and leaned on `onSelectionChange` to mirror selection into the
// store — but that lagged by one click, because React Flow's controlled selection
// stayed inconsistent until the next render. Folding the `select` deltas straight
// into the store selection here makes the FIRST click take.
import type { EdgeChange, NodeChange } from "@xyflow/react";

/**
 * Fold the `select` changes in `changes` onto the current selected-id set,
 * returning the next id list and whether anything actually changed (so the caller
 * can skip a no-op store write). Non-`select` changes are ignored. Works for both
 * node and edge changes (their `select` change shape is identical).
 */
export function applySelectionChanges(
  current: readonly string[],
  changes: readonly (NodeChange | EdgeChange)[],
): { ids: string[]; changed: boolean } {
  let changed = false;
  const set = new Set(current);
  for (const change of changes) {
    if (change.type !== "select") {
      continue;
    }
    if (change.selected) {
      if (!set.has(change.id)) {
        set.add(change.id);
        changed = true;
      }
    } else if (set.has(change.id)) {
      set.delete(change.id);
      changed = true;
    }
  }
  return { ids: changed ? [...set] : [...current], changed };
}
