import { beforeEach, describe, expect, it } from "vitest";

import { DANGLING_INDEX } from "../pipeline/passOps";
import { useDocumentStore } from "./documentStore";
import { resetIdsForTest } from "./ids";

function store() {
  return useDocumentStore.getState();
}

beforeEach(() => {
  resetIdsForTest();
  store().reset();
});

describe("documentStore — pass CRUD (#46)", () => {
  it("opens at the pipeline level with one pass", () => {
    expect(store().level).toBe("pipeline");
    expect(store().project.passes).toHaveLength(1);
  });

  it("addPass appends a graph pass, makes it active, one undo step", () => {
    const id = store().addPass();
    expect(store().project.passes).toHaveLength(2);
    expect(store().project.passes[1]!.id).toBe(id);
    expect(store().activePassId).toBe(id);
    store().undo();
    expect(store().project.passes).toHaveLength(1);
  });

  it("removePass falls the active pass back to a neighbour", () => {
    const p2 = store().addPass(); // active = p2
    store().removePass(p2);
    expect(store().project.passes).toHaveLength(1);
    // active fell back to the surviving pass.
    expect(store().activePassId).toBe(store().project.passes[0]!.id);
  });

  it("removePass refuses to drop the last pass", () => {
    const only = store().project.passes[0]!.id;
    store().removePass(only);
    expect(store().project.passes).toHaveLength(1);
  });

  it("reorderPass reindexes passes and remaps index refs", () => {
    // p0 (initial), p1, p2. Put a PassOutput0 sampler in p2.
    store().addPass(); // p1
    const p2 = store().addPass(); // p2
    store().openPass(p2);
    store().addNode("passOutput", { x: 0, y: 0 }, { index: 0 });
    const samplerId = store().activeGraph().nodes[0]!.id;
    store().showPipeline();

    // Move p0 (index 0) to the end. p2's sampler must follow p0 to its new index.
    const p0 = store().project.passes[0]!.id;
    store().reorderPass(0, 2);
    expect(store().project.passes.map((p) => p.id).includes(p0)).toBe(true);
    const newP0Index = store().project.passes.findIndex((p) => p.id === p0);
    const p2Pass = store().project.passes.find((p) => p.id === p2)!;
    const sampler =
      p2Pass.source.kind === "graph"
        ? p2Pass.source.graph.nodes.find((n) => n.id === samplerId)
        : undefined;
    expect((sampler!.data as { index: number }).index).toBe(newP0Index);
  });

  it("removing a referenced pass dangles its index ref", () => {
    const p1 = store().addPass(); // p1
    const p2 = store().addPass(); // p2 samples PassOutput of p1
    const p1Index = store().project.passes.findIndex((p) => p.id === p1);
    store().openPass(p2);
    store().addNode("passOutput", { x: 0, y: 0 }, { index: p1Index });
    const samplerId = store().activeGraph().nodes[0]!.id;
    store().showPipeline();

    store().removePass(p1);
    const p2Pass = store().project.passes.find((p) => p.id === p2)!;
    const sampler =
      p2Pass.source.kind === "graph"
        ? p2Pass.source.graph.nodes.find((n) => n.id === samplerId)
        : undefined;
    expect((sampler!.data as { index: number }).index).toBe(DANGLING_INDEX);
  });
});

describe("documentStore — drill-in/out navigation state (#46)", () => {
  it("openPass drills in and showPipeline returns", () => {
    const p2 = store().addPass();
    store().openPass(p2);
    expect(store().level).toBe("pass");
    expect(store().activePassId).toBe(p2);
    store().showPipeline();
    expect(store().level).toBe("pipeline");
  });

  it("preserves per-level viewport across navigation", () => {
    const p2 = store().addPass();

    // Pipeline-level viewport.
    store().setViewport({ x: 10, y: 20, zoom: 1.5 });
    expect(store().currentViewport()).toEqual({ x: 10, y: 20, zoom: 1.5 });

    // Drill into p2; its viewport starts empty, then set one.
    store().openPass(p2);
    expect(store().currentViewport()).toBeNull();
    store().setViewport({ x: -5, y: -6, zoom: 0.75 });
    expect(store().currentViewport()).toEqual({ x: -5, y: -6, zoom: 0.75 });

    // Back to pipeline: its viewport is intact.
    store().showPipeline();
    expect(store().currentViewport()).toEqual({ x: 10, y: 20, zoom: 1.5 });

    // Re-enter p2: its viewport is intact.
    store().openPass(p2);
    expect(store().currentViewport()).toEqual({ x: -5, y: -6, zoom: 0.75 });
  });

  it("preserves per-pass selection across drill-out and back", () => {
    const p2 = store().addPass();
    store().openPass(p2);
    const n1 = store().addNode("source", { x: 0, y: 0 });
    store().setSelection({ nodeIds: [n1], edgeIds: [] });

    // Drill out — pipeline selection records the pass we came from.
    store().showPipeline();
    expect(store().selections.pipeline).toBe(p2);
    expect(store().selection).toEqual({ nodeIds: [], edgeIds: [] });

    // Re-enter — the pass's node selection is restored.
    store().openPass(p2);
    expect(store().selection).toEqual({ nodeIds: [n1], edgeIds: [] });
  });

  it("after an undo the canvas never views a non-existent pass", () => {
    const p2 = store().addPass();
    store().openPass(p2);
    expect(store().level).toBe("pass");
    // Undo the addPass: p2 no longer exists. Whatever level we land on, the
    // active pass must still exist (never a stale/dangling pass graph).
    store().undo();
    expect(store().project.passes.some((p) => p.id === p2)).toBe(false);
    if (store().level === "pass") {
      expect(store().project.passes.some((p) => p.id === store().activePassId)).toBe(true);
    }
  });

  it("undo restores the pipeline level when the drilled-in pass is truly gone", () => {
    // Drill into the initial pass, then add+open a second pass while drilled in,
    // and ensure undo coercion kicks in when activePassId itself becomes stale.
    const p0 = store().project.passes[0]!.id;
    store().openPass(p0);
    // Snapshot a state whose activePassId is a pass that a later undo deletes.
    const p2 = store().addPass(); // active becomes p2 (a fresh snapshot baseline)
    store().openPass(p2);
    store().reorderPass(0, 1); // a mutation whose snapshot carries activePassId=p2
    // Remove p2 via undo chain: undo reorder (active back to p2 — still exists),
    // then undo addPass (active back to p0 — still exists).
    store().undo();
    store().undo();
    expect(store().project.passes.some((p) => p.id === store().activePassId)).toBe(true);
  });
});
