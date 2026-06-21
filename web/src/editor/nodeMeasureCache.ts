// Helper for the CONTROLLED editor canvas (EditorCanvas.tsx): split React Flow's
// `NodeChange[]` into the STRUCTURAL changes that belong in the document and the
// two kinds that must NOT round-trip through it — dimension MEASUREMENTS and
// SELECTION.
//
//  * `dimensions` — React Flow 12 keeps a node hidden until it has measured its
//    on-screen size, and in controlled mode it reads `node.measured` off the
//    `nodes` prop every render (`adoptUserNodes` in @xyflow/system). Our document
//    carries no node dimensions, so if a `dimensions` change is fed back through
//    the store, the next render's nodes are "unmeasured" again → React Flow
//    re-measures → emits another `dimensions` change → … an infinite update loop
//    (React error #185) that also leaves every node permanently invisible. We
//    cache measurements OUT of the document (in the map this returns into).
//  * `select` — selection is its own store field. The canvas folds `select`
//    deltas onto it directly (see selectionChanges.ts, called from
//    onNodesChange/onEdgesChange) rather than through the document graph: routing
//    `select` into the graph would rebuild the project on every click (new node
//    identities), needlessly re-triggering the debounced compile loop. So `select`
//    is filtered OUT of `structural` here too. (We do NOT use React Flow's
//    `onSelectionChange` — it lagged a click and could ping-pong.)
//
// Only position / add / remove / replace reach `applyNodeChanges`.
import type { NodeChange } from "@xyflow/react";

/** A measured node size, keyed by node id, kept outside the serialized document. */
export type MeasuredSize = { width: number; height: number };

export interface PartitionedNodeChanges {
  /** Position / add / remove / replace — applied to the document store (NOT
   * `select`, which is folded onto the store selection, NOR `dimensions`). */
  structural: NodeChange[];
  /** True when `measured` actually changed (a re-render is needed to surface it). */
  measureChanged: boolean;
}

/**
 * Fold `dimensions` changes into `measured` (mutated in place) and `remove`
 * changes out of it, returning the remaining structural changes. Pure aside from
 * the explicit, caller-owned `measured` mutation, so it is unit-testable without a
 * real React Flow canvas (jsdom never measures, so the loop this guards against is
 * invisible to component tests).
 */
export function partitionNodeChanges(
  changes: NodeChange[],
  measured: Map<string, MeasuredSize>,
): PartitionedNodeChanges {
  let measureChanged = false;
  for (const change of changes) {
    if (change.type === "dimensions") {
      if (change.dimensions) {
        const { width, height } = change.dimensions;
        const prev = measured.get(change.id);
        if (!prev || prev.width !== width || prev.height !== height) {
          measured.set(change.id, { width, height });
          measureChanged = true;
        }
      }
    } else if (change.type === "remove") {
      measured.delete(change.id);
    }
  }
  return {
    structural: changes.filter((c) => c.type !== "dimensions" && c.type !== "select"),
    measureChanged,
  };
}
