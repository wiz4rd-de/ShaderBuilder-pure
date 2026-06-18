// Store actions for whole-pass code passes (#52): switching a pass between a
// node `Graph` and opaque `WholePassCode`, and the coalesced source-edit path.
import { beforeEach, describe, expect, it } from "vitest";

import { useDocumentStore } from "./documentStore";
import { resetIdsForTest } from "./ids";

function store() {
  return useDocumentStore.getState();
}

const SRC = "#version 450\n#pragma stage fragment\nvoid main() {}\n";

beforeEach(() => {
  resetIdsForTest();
  store().reset();
});

describe("documentStore — whole-pass code switching (#52)", () => {
  it("setPassToWholePassCode makes a pass opaque, one undo entry", () => {
    const passId = store().project.passes[0]!.id;
    store().setPassToWholePassCode(passId, SRC);
    const pass = store().project.passes[0]!;
    expect(pass.source.kind).toBe("wholePassCode");
    if (pass.source.kind === "wholePassCode") {
      expect(pass.source.source).toBe(SRC);
      expect(pass.source.opaque).toBe(true);
    }
    expect(store().dirty).toBe(true);
    // Reversible: undo restores the graph pass.
    store().undo();
    expect(store().project.passes[0]!.source.kind).toBe("graph");
  });

  it("setPassToWholePassCode clears a stale node selection in the active pass", () => {
    const passId = store().project.passes[0]!.id;
    store().openPass(passId);
    const nodeId = store().addNode("source", { x: 0, y: 0 });
    store().setSelection({ nodeIds: [nodeId], edgeIds: [] });
    store().setPassToWholePassCode(passId, SRC);
    expect(store().selection.nodeIds).toEqual([]);
  });

  it("setPassToGraph converts an opaque pass back to an empty graph", () => {
    const passId = store().project.passes[0]!.id;
    store().setPassToWholePassCode(passId, SRC);
    store().setPassToGraph(passId);
    const pass = store().project.passes[0]!;
    expect(pass.source.kind).toBe("graph");
    if (pass.source.kind === "graph") {
      expect(pass.source.graph).toEqual({ nodes: [], edges: [] });
    }
  });

  it("patchWholePassSource edits source live; coalesces under one undo entry", () => {
    const passId = store().project.passes[0]!.id;
    store().setPassToWholePassCode(passId, SRC);
    const undoDepth = store().past.length;

    store().beginInteraction();
    store().patchWholePassSource(passId, "a");
    store().patchWholePassSource(passId, "ab");
    store().patchWholePassSource(passId, "abc");
    store().commit();

    const pass = store().project.passes[0]!;
    if (pass.source.kind === "wholePassCode") {
      expect(pass.source.source).toBe("abc");
    }
    // The whole typing burst is exactly ONE new undo entry.
    expect(store().past.length).toBe(undoDepth + 1);
    store().undo();
    const reverted = store().project.passes[0]!;
    if (reverted.source.kind === "wholePassCode") {
      expect(reverted.source.source).toBe(SRC);
    }
  });

  it("patchWholePassSource is a no-op on a graph pass", () => {
    const passId = store().project.passes[0]!.id;
    store().patchWholePassSource(passId, "ignored");
    expect(store().project.passes[0]!.source.kind).toBe("graph");
  });
});
