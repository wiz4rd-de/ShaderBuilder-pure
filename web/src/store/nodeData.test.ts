import { beforeEach, describe, expect, it } from "vitest";

import type { Graph } from "../bindings/Graph";
import { useDocumentStore } from "./documentStore";
import { resetIdsForTest } from "./ids";

function store() {
  return useDocumentStore.getState();
}
function graph(): Graph {
  return store().activeGraph();
}
function nodeData(id: string): Record<string, unknown> {
  return graph().nodes.find((n) => n.id === id)!.data;
}

beforeEach(() => {
  resetIdsForTest();
  store().reset();
});

describe("documentStore — updateNodeData", () => {
  it("merges a patch into a node's data as one undo entry", () => {
    const id = store().addNode("const", { x: 0, y: 0 }, { constType: "float", value: 0 });
    store().updateNodeData(id, { value: 0.5 });
    expect(nodeData(id)).toEqual({ constType: "float", value: 0.5 });
    expect(store().canUndo()).toBe(true);

    store().undo();
    expect(nodeData(id).value).toBe(0);
  });

  it("deletes a key when the patch value is undefined", () => {
    const id = store().addNode("const", { x: 0, y: 0 }, { constType: "vec2", value: [1, 2] });
    store().updateNodeData(id, { value: undefined });
    expect("value" in nodeData(id)).toBe(false);
  });

  it("is a no-op (no history entry) when the patch changes nothing", () => {
    const id = store().addNode("const", { x: 0, y: 0 }, { constType: "float", value: 1 });
    const before = store().past.length;
    store().updateNodeData(id, { value: 1, constType: "float" });
    expect(store().past.length).toBe(before);
  });

  it("does not mutate the prior snapshot's data (structural sharing is safe)", () => {
    const id = store().addNode("const", { x: 0, y: 0 }, { value: 1 });
    store().updateNodeData(id, { value: 2 });
    store().undo();
    expect(nodeData(id).value).toBe(1);
    store().redo();
    expect(nodeData(id).value).toBe(2);
  });
});

describe("documentStore — patchNodeData (live, non-committing)", () => {
  it("applies live without pushing history; commit coalesces one entry", () => {
    const id = store().addNode("const", { x: 0, y: 0 }, { value: 0 });
    store().beginInteraction();
    store().patchNodeData(id, { value: 1 });
    store().patchNodeData(id, { value: 2 });
    store().patchNodeData(id, { value: 3 });
    expect(nodeData(id).value).toBe(3);
    store().commit();
    expect(store().canUndo()).toBe(true);

    // One undo reverts the whole coalesced burst back to the baseline.
    store().undo();
    expect(nodeData(id).value).toBe(0);
  });
});

describe("documentStore — diagnosticsByNode", () => {
  it("starts empty and is replaceable via the setter", () => {
    expect(store().diagnosticsByNode).toEqual({});
    store().setDiagnosticsByNode({
      n1: [{ severity: "error", code: "typeMismatch", message: "bad", node: "n1", port: "coord" }],
    });
    expect(store().diagnosticsByNode.n1).toHaveLength(1);
  });

  it("clears on reset", () => {
    store().setDiagnosticsByNode({ n1: [] });
    store().reset();
    expect(store().diagnosticsByNode).toEqual({});
  });
});
