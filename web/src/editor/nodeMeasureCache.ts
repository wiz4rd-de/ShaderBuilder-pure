// Helper for the CONTROLLED editor canvas (EditorCanvas.tsx): split React Flow's
// `NodeChange[]` into the structural changes that belong in the document and the
// dimension MEASUREMENTS that must NOT.
//
// React Flow 12 keeps a node hidden until it has measured its on-screen size, and
// in controlled mode it reads `node.measured` off the `nodes` prop every render
// (`adoptUserNodes` in @xyflow/system). Our document carries no node dimensions,
// so if a `dimensions` change is fed back through the store, the next render's
// nodes are "unmeasured" again → React Flow re-measures → emits another
// `dimensions` change → … an infinite update loop (React error #185) that also
// leaves every node permanently invisible. So we cache measurements OUT of the
// document (in the map this returns into) and only send structural changes
// (position / select / remove) to the store.
import type { NodeChange } from "@xyflow/react";

/** A measured node size, keyed by node id, kept outside the serialized document. */
export type MeasuredSize = { width: number; height: number };

export interface PartitionedNodeChanges {
  /** Position / select / remove — applied to the document store. */
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
    structural: changes.filter((c) => c.type !== "dimensions"),
    measureChanged,
  };
}
