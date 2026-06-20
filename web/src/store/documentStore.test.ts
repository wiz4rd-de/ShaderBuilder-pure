import { beforeEach, describe, expect, it } from "vitest";

import type { Graph } from "../bindings/Graph";
import { useDocumentStore } from "./documentStore";
import { resetIdsForTest } from "./ids";

// Direct, headless tests against the zustand store (no React render needed).
// getState() exposes the full action surface; we drive it like a reducer.
function store() {
  return useDocumentStore.getState();
}

function graph(): Graph {
  return store().activeGraph();
}

beforeEach(() => {
  resetIdsForTest();
  store().reset();
});

describe("documentStore — structure", () => {
  it("opens with a single empty graph pass and a clean history", () => {
    expect(store().project.passes).toHaveLength(1);
    expect(graph().nodes).toHaveLength(0);
    expect(graph().edges).toHaveLength(0);
    expect(store().canUndo()).toBe(false);
    expect(store().canRedo()).toBe(false);
    expect(store().dirty).toBe(false);
  });

  it("serializes to the core-model schema and round-trips through JSON", () => {
    const a = store().addNode("placeholder", { x: 10, y: 20 }, { k: 1 });
    const b = store().addNode("placeholder", { x: 30, y: 40 });
    store().addEdge(a, "out", b, "in");

    const snap = store().toSnapshot();
    const roundTripped = JSON.parse(JSON.stringify(snap));
    expect(roundTripped).toEqual(snap);

    // The active graph survives the round-trip with all ids/positions/edges.
    const rtGraph = roundTripped.project.passes.find(
      (p: { id: string }) => p.id === roundTripped.activePassId,
    ).source.graph as Graph;
    expect(rtGraph.nodes).toHaveLength(2);
    expect(rtGraph.edges).toHaveLength(1);
    expect(rtGraph.edges[0]!.source).toBe(a);
    expect(rtGraph.edges[0]!.target).toBe(b);
  });
});

describe("documentStore — add / move / delete undo-redo", () => {
  it("undo/redo of addNode", () => {
    store().addNode("placeholder", { x: 0, y: 0 });
    expect(graph().nodes).toHaveLength(1);
    store().undo();
    expect(graph().nodes).toHaveLength(0);
    store().redo();
    expect(graph().nodes).toHaveLength(1);
  });

  it("undo/redo of moveNodes restores the exact prior position", () => {
    const id = store().addNode("placeholder", { x: 0, y: 0 });
    store().moveNodes([{ id, position: { x: 100, y: 200 } }]);
    expect(graph().nodes[0]!.position).toEqual({ x: 100, y: 200 });
    store().undo();
    expect(graph().nodes[0]!.position).toEqual({ x: 0, y: 0 });
    store().redo();
    expect(graph().nodes[0]!.position).toEqual({ x: 100, y: 200 });
  });

  it("undo/redo of delete restores nodes AND incident edges", () => {
    const a = store().addNode("placeholder", { x: 0, y: 0 });
    const b = store().addNode("placeholder", { x: 50, y: 0 });
    store().addEdge(a, "out", b, "in");
    store().setSelection({ nodeIds: [a], edgeIds: [] });
    store().removeSelection();
    // Deleting node a also removed the incident edge.
    expect(graph().nodes.map((n) => n.id)).toEqual([b]);
    expect(graph().edges).toHaveLength(0);

    store().undo();
    expect(graph().nodes.map((n) => n.id).sort()).toEqual([a, b].sort());
    expect(graph().edges).toHaveLength(1);
    expect(graph().edges[0]!.source).toBe(a);

    store().redo();
    expect(graph().nodes.map((n) => n.id)).toEqual([b]);
    expect(graph().edges).toHaveLength(0);
  });
});

describe("documentStore — copy / paste / duplicate", () => {
  it("paste re-maps ids and re-points internal edges only", () => {
    const a = store().addNode("placeholder", { x: 0, y: 0 });
    const b = store().addNode("placeholder", { x: 60, y: 0 });
    store().addEdge(a, "out", b, "in");

    store().setSelection({ nodeIds: [a, b], edgeIds: [] });
    store().copy();
    store().paste();

    // Two originals + two pasted.
    expect(graph().nodes).toHaveLength(4);
    expect(graph().edges).toHaveLength(2);

    const pastedIds = new Set(store().selection.nodeIds);
    expect(pastedIds.size).toBe(2);
    expect(pastedIds.has(a)).toBe(false);
    expect(pastedIds.has(b)).toBe(false);

    // The pasted edge connects only the pasted nodes.
    const pastedEdge = graph().edges.find(
      (e) => pastedIds.has(e.source) && pastedIds.has(e.target),
    );
    expect(pastedEdge).toBeDefined();
    expect(pastedEdge!.source).not.toBe(a);
    expect(pastedEdge!.target).not.toBe(b);
  });

  it("paste is a single undo step", () => {
    const a = store().addNode("placeholder", { x: 0, y: 0 });
    store().addNode("placeholder", { x: 60, y: 0 });
    store().setSelection({ nodeIds: [a], edgeIds: [] });
    store().copy();
    store().paste();
    expect(graph().nodes).toHaveLength(3);
    store().undo();
    expect(graph().nodes).toHaveLength(2);
  });

  it("duplicate offsets and re-ids without needing the clipboard", () => {
    const a = store().addNode("placeholder", { x: 5, y: 5 });
    store().setSelection({ nodeIds: [a], edgeIds: [] });
    store().duplicate();
    expect(graph().nodes).toHaveLength(2);
    const dup = graph().nodes.find((n) => n.id !== a)!;
    expect(dup.position).toEqual({ x: 37, y: 37 });
  });
});

describe("documentStore — drag coalescing", () => {
  it("a drag (begin → live moves → commit) is ONE undo step", () => {
    const id = store().addNode("placeholder", { x: 0, y: 0 });

    store().beginInteraction();
    // Simulate React Flow streaming live position changes during the drag.
    store().applyNodeChanges([
      { type: "position", id, position: { x: 5, y: 5 }, dragging: true },
    ]);
    store().applyNodeChanges([
      { type: "position", id, position: { x: 40, y: 80 }, dragging: true },
    ]);
    store().applyNodeChanges([
      { type: "position", id, position: { x: 90, y: 120 }, dragging: false },
    ]);
    store().commit();

    expect(graph().nodes[0]!.position).toEqual({ x: 90, y: 120 });

    // One undo returns to the pre-drag position — not an intermediate frame.
    store().undo();
    expect(graph().nodes[0]!.position).toEqual({ x: 0, y: 0 });
    store().redo();
    expect(graph().nodes[0]!.position).toEqual({ x: 90, y: 120 });
  });

  it("a no-op interaction (begin then commit, no change) adds no history", () => {
    store().addNode("placeholder", { x: 0, y: 0 });
    const depth = store().past.length;
    store().beginInteraction();
    store().commit();
    expect(store().past.length).toBe(depth);
  });
});

describe("documentStore — ≥20 mixed operations, exact-state undo/redo", () => {
  it("undoing back to start reproduces the initial empty snapshot byte-for-byte", () => {
    const snapshots: string[] = [];
    const record = () => snapshots.push(JSON.stringify(store().toSnapshot()));

    record(); // s0: empty

    // A deterministic, mixed sequence of >20 operations. Each operation that
    // mutates the document records the resulting snapshot so we can walk back.
    const ids: string[] = [];
    for (let i = 0; i < 6; i += 1) {
      ids.push(store().addNode("placeholder", { x: i * 10, y: i * 5 }, { i }));
      record();
    }
    // edges
    store().addEdge(ids[0]!, "out", ids[1]!, "in");
    record();
    store().addEdge(ids[1]!, "out", ids[2]!, "in");
    record();
    store().addEdge(ids[2]!, "out", ids[3]!, "in");
    record();
    // moves
    store().moveNodes([{ id: ids[0]!, position: { x: 500, y: 500 } }]);
    record();
    store().moveNodes([
      { id: ids[1]!, position: { x: 1, y: 2 } },
      { id: ids[2]!, position: { x: 3, y: 4 } },
    ]);
    record();
    // copy/paste
    store().setSelection({ nodeIds: [ids[0]!, ids[1]!], edgeIds: [] });
    store().copy();
    store().paste();
    record();
    // duplicate
    store().setSelection({ nodeIds: [ids[3]!], edgeIds: [] });
    store().duplicate();
    record();
    // a coalesced drag
    store().beginInteraction();
    store().applyNodeChanges([
      { type: "position", id: ids[4]!, position: { x: 7, y: 7 }, dragging: true },
    ]);
    store().applyNodeChanges([
      { type: "position", id: ids[4]!, position: { x: 77, y: 88 }, dragging: false },
    ]);
    store().commit();
    record();
    // delete a node (and its incident edges)
    store().setSelection({ nodeIds: [ids[2]!], edgeIds: [] });
    store().removeSelection();
    record();
    // another add
    const extra1 = store().addNode("placeholder", { x: 9, y: 9 });
    record();
    // more moves to push the count past 20 mixed operations
    store().moveNodes([{ id: extra1, position: { x: 11, y: 22 } }]);
    record();
    store().moveNodes([{ id: ids[0]!, position: { x: 33, y: 44 } }]);
    record();
    // another edge
    store().addEdge(ids[3]!, "out", extra1, "in");
    record();
    // duplicate the lot
    store().setSelection({ nodeIds: [ids[3]!, extra1], edgeIds: [] });
    store().duplicate();
    record();
    // delete by selection again
    store().setSelection({ nodeIds: [extra1], edgeIds: [] });
    store().removeSelection();
    record();
    // final add
    store().addNode("placeholder", { x: 1, y: 1 });
    record();

    // We have well over 20 mutating operations recorded.
    expect(snapshots.length).toBeGreaterThan(20);

    // Walk the ENTIRE history backwards; each undo must reproduce the EXACT
    // snapshot taken just before the corresponding operation.
    for (let i = snapshots.length - 1; i >= 1; i -= 1) {
      store().undo();
      expect(JSON.stringify(store().toSnapshot())).toBe(snapshots[i - 1]);
    }
    // Back at the start.
    expect(JSON.stringify(store().toSnapshot())).toBe(snapshots[0]);
    expect(store().canUndo()).toBe(false);

    // Now redo all the way forward; each redo reproduces the matching snapshot.
    for (let i = 1; i < snapshots.length; i += 1) {
      store().redo();
      expect(JSON.stringify(store().toSnapshot())).toBe(snapshots[i]);
    }
    expect(store().canRedo()).toBe(false);
  });
});

describe("documentStore — engine problems vs compile (#14)", () => {
  const cleanCompile = {
    diagnosticsByNode: {},
    problems: [],
    valid: true,
    sourcesByPassId: {},
  };

  it("preserves a device-level engine problem across a successful compile", () => {
    // A pipeline-wide device failure: no passId, code deviceLost.
    store().pushEngineProblem({
      severity: "error",
      code: "deviceLost",
      message: "the GPU device was lost",
      passId: null,
      nodeId: null,
    });
    store().setEngineStatus("stopped");
    expect(store().engineProblems).toHaveLength(1);

    // A subsequent successful compile (an unrelated edit) must NOT clear the
    // device problem while the engine is still stopped — dispatchPreview only
    // re-emits slangCompile, so it could never re-derive this.
    store().setCompileStatus(cleanCompile);

    expect(store().engineProblems).toHaveLength(1);
    expect(store().engineProblems[0]!.diagnostic.code).toBe("deviceLost");
    expect(store().engineStatus).toBe("stopped");
  });

  it("clears a pass-level slangCompile engine problem on the next compile", () => {
    // A pass-tagged compile failure: dispatchPreview re-establishes it, so the
    // stale one must be cleared by the fresh compile.
    store().pushEngineProblem({
      severity: "error",
      code: "slangCompile",
      message: "syntax error",
      passId: "pass-1",
      nodeId: null,
    });
    expect(store().engineProblems).toHaveLength(1);

    store().setCompileStatus(cleanCompile);

    expect(store().engineProblems).toHaveLength(0);
  });
});
