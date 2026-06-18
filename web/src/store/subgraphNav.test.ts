// Fix C (#57): an interior edit that deletes a node referenced by a subgraph
// boundary port must RECONCILE the boundary ports AND drop the now-dangling
// EXTERIOR edge in the SAME mutation — so the disconnection is visible in the
// editor immediately, instead of an edge that survives only to be SILENTLY
// dropped by graphToIr's inlining step at compile time.
import { beforeEach, describe, expect, it } from "vitest";

import type { Subgraph } from "../bindings/Subgraph";
import { graphToIr } from "../nodes/graphToIr";
import { isSubgraphNode, readSubgraph } from "../nodes/subgraph";
import { useDocumentStore } from "./documentStore";
import { resetIdsForTest } from "./ids";

function store() {
  return useDocumentStore.getState();
}

beforeEach(() => {
  resetIdsForTest();
  store().reset();
});

describe("subgraph boundary reconciliation on interior edit (#57, Fix C)", () => {
  it("deleting an interior node prunes its boundary port AND the parent edge", () => {
    // Build: texcoord -> source -> output. Collapse {texcoord, source}: the only
    // crossing edge is source.out -> output.color, so the wrapper gets ONE `out`
    // boundary port bound to the interior `source` node.
    const passId = store().project.passes[0]!.id;
    store().openPass(passId);
    const tc = store().addNode("texcoord", { x: 0, y: 0 });
    const src = store().addNode("source", { x: 100, y: 0 });
    const out = store().addNode("output", { x: 200, y: 0 });
    store().addEdge(tc, "uv", src, "coord");
    store().addEdge(src, "out", out, "color");

    store().setSelection({ nodeIds: [tc, src], edgeIds: [] });
    store().collapseSelection("Inner");
    const sgId = store().activeGraph().nodes.find(isSubgraphNode)!.id;

    // The wrapper advertises one `out` boundary port; a parent edge connects it
    // to output.color. Capture the boundary port name + that parent edge.
    const passGraph0 = store().activeGraph();
    const wrapper0 = passGraph0.nodes.find((n) => n.id === sgId)!;
    const sub0 = readSubgraph(wrapper0);
    const outPort = sub0.boundaryPorts.find((b) => b.direction === "out")!;
    expect(outPort.interiorNode).toBe(src);
    const parentEdge0 = passGraph0.edges.find(
      (e) => e.source === sgId && e.sourcePort === outPort.name,
    );
    expect(parentEdge0).toBeTruthy();
    expect(parentEdge0!.target).toBe(out);

    // Drill in and DELETE the interior `source` node the boundary port references.
    store().openSubgraph(sgId);
    expect(store().subgraphPath).toEqual([sgId]);
    store().setSelection({ nodeIds: [src], edgeIds: [] });
    store().removeSelection();
    store().closeSubgraph();

    // Post-condition 1: the boundary port that referenced `source` is GONE.
    const passGraph1 = store().activeGraph();
    const wrapper1 = passGraph1.nodes.find((n) => n.id === sgId)!;
    const sub1: Subgraph = readSubgraph(wrapper1);
    expect(sub1.nodes.some((n) => n.id === src)).toBe(false); // interior node gone
    expect(sub1.boundaryPorts.some((b) => b.name === outPort.name)).toBe(false);

    // Post-condition 2: the EXTERIOR edge that referenced the pruned port is GONE
    // (not silently surviving to be dropped at compile time).
    expect(
      passGraph1.edges.some((e) => e.source === sgId && e.sourcePort === outPort.name),
    ).toBe(false);
    // The output node itself survives, just disconnected.
    expect(passGraph1.nodes.some((n) => n.id === out)).toBe(true);

    // Post-condition 3: graphToIr of the result drops NO edge — every IR edge's
    // endpoints are live IR nodes (no orphaned/silently-dropped edge). Because the
    // dangling parent edge was already pruned in the editor, inlining never has to
    // silently discard one: the IR edge count equals the editor edge count.
    const { ir } = graphToIr(passGraph1);
    const liveIds = new Set(ir.nodes.map((n) => n.id));
    for (const e of ir.edges) {
      expect(liveIds.has(e.source.node)).toBe(true);
      expect(liveIds.has(e.target.node)).toBe(true);
    }
    expect(ir.edges.length).toBe(passGraph1.edges.length);
  });

  it("an interior edit that does NOT touch any boundary leaves ports + edges intact", () => {
    // Regression guard: reconciliation must not prune live boundaries. Collapse a
    // chain, drill in, add an unrelated node — the boundary + parent edge persist.
    const passId = store().project.passes[0]!.id;
    store().openPass(passId);
    const tc = store().addNode("texcoord", { x: 0, y: 0 });
    const src = store().addNode("source", { x: 100, y: 0 });
    const out = store().addNode("output", { x: 200, y: 0 });
    store().addEdge(tc, "uv", src, "coord");
    store().addEdge(src, "out", out, "color");
    store().setSelection({ nodeIds: [tc, src], edgeIds: [] });
    store().collapseSelection("Inner");
    const sgId = store().activeGraph().nodes.find(isSubgraphNode)!.id;
    const outPort = readSubgraph(
      store().activeGraph().nodes.find((n) => n.id === sgId)!,
    ).boundaryPorts.find((b) => b.direction === "out")!;

    store().openSubgraph(sgId);
    store().addNode("texcoord", { x: -50, y: -50 }); // unrelated interior edit
    store().closeSubgraph();

    const passGraph = store().activeGraph();
    const sub = readSubgraph(passGraph.nodes.find((n) => n.id === sgId)!);
    expect(sub.boundaryPorts.some((b) => b.name === outPort.name)).toBe(true);
    expect(
      passGraph.edges.some((e) => e.source === sgId && e.sourcePort === outPort.name),
    ).toBe(true);
  });
});
