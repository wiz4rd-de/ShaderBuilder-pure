// Pure TypeScript mirror of `LibraryItem::instantiate` (#56) — the ONE id-
// remapping algorithm shared by Rust and the frontend. Inserting a library item
// into a graph must mint FRESH ids so two inserts of the same item never clash;
// this reproduces the documented Rust contract exactly so the two stay in lock-
// step (see crates/core-model/src/lib.rs `LibraryItem::instantiate`).
//
//   - Node payload: clone the node, give it a fresh id.
//   - Subgraph payload:
//       1. give the Subgraph body a fresh id;
//       2. build an old-node-id -> new-node-id map (a fresh id per interior node);
//       3. rewrite every interior edge's source/target through the map + fresh
//          edge ids;
//       4. rewrite every BoundaryPort.interiorNode through the same map
//          (name + interiorPort are stable port NAMES, left untouched).
//
// The WRAPPING `kind === "subgraph"` Node's own id is NOT minted here — the
// caller/store assigns it when it drops the node on the canvas (mirroring Rust,
// which returns only the fresh Subgraph body for that node's `data`).
import type { Edge } from "../bindings/Edge";
import type { LibraryItem } from "../bindings/LibraryItem";
import type { LibraryPayload } from "../bindings/LibraryPayload";
import type { Subgraph } from "../bindings/Subgraph";
import type { MintId } from "../nodes/subgraph";

/**
 * Clone an item's payload with fresh ids (the #56 contract, in TS). `mintId`
 * mints fresh, globally-unique ids (e.g. the store's `nextId`).
 */
export function instantiateLibraryItem(
  item: LibraryItem,
  mintId: MintId,
): LibraryPayload {
  const payload = item.payload;
  if (payload.kind === "node") {
    return {
      kind: "node",
      node: {
        ...payload.node,
        id: mintId("node"),
        position: { ...payload.node.position },
        data: structuredClone(payload.node.data),
      },
    };
  }

  const src = payload.subgraph;
  // (2) old node id -> fresh id, minting one per interior node.
  const idMap = new Map<string, string>();
  const nodes = src.nodes.map((n) => {
    const freshId = mintId("node");
    idMap.set(n.id, freshId);
    return {
      ...n,
      id: freshId,
      position: { ...n.position },
      data: structuredClone(n.data),
    };
  });

  // (3) rewrite interior edge endpoints through the map + fresh edge ids.
  const edges: Edge[] = src.edges.map((e) => ({
    id: mintId("edge"),
    source: idMap.get(e.source) ?? e.source,
    sourcePort: e.sourcePort,
    target: idMap.get(e.target) ?? e.target,
    targetPort: e.targetPort,
  }));

  // (4) rewrite boundary-port interior node ids (names stay stable).
  const boundaryPorts = src.boundaryPorts.map((bp) => ({
    ...bp,
    interiorNode: idMap.get(bp.interiorNode) ?? bp.interiorNode,
  }));

  const subgraph: Subgraph = {
    id: mintId("subgraph"), // (1) fresh body id
    name: src.name,
    nodes,
    edges,
    boundaryPorts,
  };
  return { kind: "subgraph", subgraph };
}
