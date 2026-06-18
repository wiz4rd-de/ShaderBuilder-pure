import { beforeEach, describe, expect, it } from "vitest";

import type { Subgraph } from "../bindings/Subgraph";
import { isSubgraphNode } from "../nodes/subgraph";
import { useDocumentStore } from "./documentStore";
import { resetIdsForTest } from "./ids";

function store() {
  return useDocumentStore.getState();
}

beforeEach(() => {
  resetIdsForTest();
  store().reset();
});

/** Drill into the only pass and build Texcoord → Sample(Source) → Output. */
function buildChain(): { tc: string; src: string; out: string; passId: string } {
  const passId = store().project.passes[0]!.id;
  store().openPass(passId);
  const tc = store().addNode("texcoord", { x: 0, y: 0 });
  const src = store().addNode("source", { x: 100, y: 0 });
  const out = store().addNode("output", { x: 200, y: 0 });
  store().addEdge(tc, "uv", src, "coord");
  store().addEdge(src, "out", out, "color");
  return { tc, src, out, passId };
}

describe("documentStore — collapseSelection / expandSubgraphNode (#57)", () => {
  it("collapses a selection into one subgraph node (one undo entry)", () => {
    const { src } = buildChain();
    store().setSelection({ nodeIds: [src], edgeIds: [] });
    store().collapseSelection("MySub");

    const g = store().activeGraph();
    expect(g.nodes.filter(isSubgraphNode)).toHaveLength(1);
    expect(g.nodes.some((n) => n.id === src)).toBe(false);
    const sgId = g.nodes.find(isSubgraphNode)!.id;
    expect(store().selection.nodeIds).toEqual([sgId]);

    // Undo restores the un-collapsed graph in one step.
    store().undo();
    const restored = store().activeGraph();
    expect(restored.nodes.some(isSubgraphNode)).toBe(false);
    expect(restored.nodes.some((n) => n.id === src)).toBe(true);
  });

  it("collapse then expand round-trips to a structurally-equivalent graph", () => {
    const { tc, src, out } = buildChain();
    store().setSelection({ nodeIds: [src], edgeIds: [] });
    store().collapseSelection("S");
    const sgId = store().activeGraph().nodes.find(isSubgraphNode)!.id;

    store().expandSubgraphNode(sgId);
    const g = store().activeGraph();
    expect(g.nodes.some(isSubgraphNode)).toBe(false);
    // Same node kinds present (modulo ids).
    expect(g.nodes.map((n) => n.kind).sort()).toEqual(["output", "source", "texcoord"]);
    // The edge topology is preserved: a coord feed and a color feed exist.
    const kindById = new Map(g.nodes.map((n) => [n.id, n.kind]));
    const sigs = g.edges
      .map((e) => `${kindById.get(e.source)}#${e.sourcePort}->${kindById.get(e.target)}#${e.targetPort}`)
      .sort();
    expect(sigs).toEqual(["source#out->output#color", "texcoord#uv->source#coord"]);
    // (the original tc/out ids still anchor the chain)
    expect(g.nodes.some((n) => n.id === tc)).toBe(true);
    expect(g.nodes.some((n) => n.id === out)).toBe(true);
  });
});

describe("documentStore — subgraph drill-in (#57)", () => {
  it("opens a subgraph interior as the active graph and edits persist into data", () => {
    const { src } = buildChain();
    store().setSelection({ nodeIds: [src], edgeIds: [] });
    store().collapseSelection("Inner");
    const sgId = store().activeGraph().nodes.find(isSubgraphNode)!.id;

    // Drill in: the active graph is now the interior (the lone Source sample).
    store().openSubgraph(sgId);
    expect(store().subgraphPath).toEqual([sgId]);
    const interior = store().activeGraph();
    expect(interior.nodes.map((n) => n.id)).toEqual([src]);

    // Add a node inside the interior — it must persist into the subgraph data.
    const extra = store().addNode("texcoord", { x: -50, y: -50 });
    expect(store().activeGraph().nodes.some((n) => n.id === extra)).toBe(true);

    // Drill back out: the pass graph still shows just the subgraph node, whose
    // data now carries the edited interior.
    store().closeSubgraph();
    expect(store().subgraphPath).toEqual([]);
    const passGraph = store().activeGraph();
    const sgNode = passGraph.nodes.find((n) => n.id === sgId)!;
    const sub = sgNode.data as unknown as Subgraph;
    expect(sub.nodes.some((n) => n.id === extra)).toBe(true);
  });

  it("renaming via the inspector field updates the collapsed node label (data.name)", () => {
    const { src } = buildChain();
    store().setSelection({ nodeIds: [src], edgeIds: [] });
    store().collapseSelection("Before");
    const sgId = store().activeGraph().nodes.find(isSubgraphNode)!.id;

    // The descriptor's editable `name` field writes data.name.
    store().updateNodeData(sgId, { name: "Renamed" });
    const sgNode = store().activeGraph().nodes.find((n) => n.id === sgId)!;
    expect((sgNode.data as unknown as Subgraph).name).toBe("Renamed");
  });

  it("openSubgraph is a no-op for a non-subgraph node", () => {
    const { src } = buildChain();
    store().openSubgraph(src);
    expect(store().subgraphPath).toEqual([]);
  });

  it("undoing a collapse while drilled into it trims the path back to the pass graph", () => {
    const { src } = buildChain();
    store().setSelection({ nodeIds: [src], edgeIds: [] });
    store().collapseSelection("X");
    const sgId = store().activeGraph().nodes.find(isSubgraphNode)!.id;
    store().openSubgraph(sgId);
    expect(store().subgraphPath).toEqual([sgId]);

    // Undo removes the subgraph node; the drill-in path must trim to empty.
    store().undo();
    expect(store().subgraphPath).toEqual([]);
    expect(store().activeGraph().nodes.some((n) => n.id === src)).toBe(true);
  });
});
