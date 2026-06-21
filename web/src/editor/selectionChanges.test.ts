import type { EdgeChange, NodeChange } from "@xyflow/react";
import { describe, expect, it } from "vitest";

import { applySelectionChanges } from "./selectionChanges";

// Locks the controlled-selection contract that fixes the "needs two clicks" bug:
// React Flow's `select` deltas must fold onto the store selection so the FIRST
// click registers. (The real one-click behaviour can't be exercised in jsdom,
// which doesn't render the canvas — this guards the fold logic instead.)
describe("applySelectionChanges", () => {
  it("selects a freshly-clicked node on the first change (added to the set)", () => {
    const changes: NodeChange[] = [{ id: "node-1", type: "select", selected: true }];
    const { ids, changed } = applySelectionChanges([], changes);
    expect(changed).toBe(true);
    expect(ids).toEqual(["node-1"]);
  });

  it("supports multi-select deltas in one batch (select one, deselect another)", () => {
    const changes: NodeChange[] = [
      { id: "b", type: "select", selected: true },
      { id: "a", type: "select", selected: false },
    ];
    const { ids, changed } = applySelectionChanges(["a"], changes);
    expect(changed).toBe(true);
    expect(ids.sort()).toEqual(["b"]);
  });

  it("reports no change when the selection is already in the requested state", () => {
    const changes: NodeChange[] = [{ id: "a", type: "select", selected: true }];
    const { ids, changed } = applySelectionChanges(["a"], changes);
    expect(changed).toBe(false);
    expect(ids).toEqual(["a"]); // returns the current ids unchanged
  });

  it("ignores non-select changes (position/remove/dimensions)", () => {
    const changes: NodeChange[] = [
      { id: "a", type: "position", position: { x: 1, y: 2 }, dragging: true },
      { id: "a", type: "remove" },
      { id: "a", type: "dimensions", dimensions: { width: 10, height: 10 } },
    ];
    const { changed } = applySelectionChanges(["a"], changes);
    expect(changed).toBe(false);
  });

  it("works for edge select changes too", () => {
    const changes: EdgeChange[] = [{ id: "e1", type: "select", selected: true }];
    const { ids, changed } = applySelectionChanges([], changes);
    expect(changed).toBe(true);
    expect(ids).toEqual(["e1"]);
  });
});
