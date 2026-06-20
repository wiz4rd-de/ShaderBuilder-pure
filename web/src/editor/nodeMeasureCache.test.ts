import type { NodeChange } from "@xyflow/react";
import { describe, expect, it } from "vitest";

import { type MeasuredSize, partitionNodeChanges } from "./nodeMeasureCache";

// Regression guard for the React #185 infinite-loop bug (nodes invisible in the
// per-pass editor). The canvas is CONTROLLED, so React Flow's `dimensions`
// measurements must be cached out-of-document and NEVER routed back to the store;
// only structural changes (position / select / remove) belong there. jsdom never
// measures the canvas, so the loop itself can't be reproduced in a component
// test — this locks the partition contract that prevents it instead.
describe("partitionNodeChanges", () => {
  it("caches dimensions out-of-document and excludes them from structural changes", () => {
    const measured = new Map<string, MeasuredSize>();
    const changes: NodeChange[] = [
      { id: "node-1", type: "dimensions", dimensions: { width: 132, height: 56 } },
    ];

    const { structural, measureChanged } = partitionNodeChanges(changes, measured);

    expect(structural).toHaveLength(0); // must NOT round-trip to the store
    expect(measureChanged).toBe(true);
    expect(measured.get("node-1")).toEqual({ width: 132, height: 56 });
  });

  it("reports no change when a re-measure yields the same size (loop break)", () => {
    const measured = new Map<string, MeasuredSize>([["node-1", { width: 132, height: 56 }]]);
    const changes: NodeChange[] = [
      { id: "node-1", type: "dimensions", dimensions: { width: 132, height: 56 } },
    ];

    const { measureChanged } = partitionNodeChanges(changes, measured);

    // A measurement equal to the cached one must be a no-op: this is exactly the
    // condition that lets React Flow settle instead of re-rendering forever.
    expect(measureChanged).toBe(false);
  });

  it("keeps structural changes (position/select/remove) and drops removed measurements", () => {
    const measured = new Map<string, MeasuredSize>([["node-1", { width: 132, height: 56 }]]);
    const changes: NodeChange[] = [
      { id: "node-1", type: "position", position: { x: 10, y: 20 }, dragging: true },
      { id: "node-2", type: "select", selected: true },
      { id: "node-1", type: "remove" },
    ];

    const { structural } = partitionNodeChanges(changes, measured);

    expect(structural.map((c) => c.type)).toEqual(["position", "select", "remove"]);
    expect(measured.has("node-1")).toBe(false); // removal prunes the cache entry
  });

  it("ignores a dimensions change with no payload", () => {
    const measured = new Map<string, MeasuredSize>();
    const changes: NodeChange[] = [{ id: "node-1", type: "dimensions" }];

    const { structural, measureChanged } = partitionNodeChanges(changes, measured);

    expect(structural).toHaveLength(0);
    expect(measureChanged).toBe(false);
    expect(measured.size).toBe(0);
  });
});
