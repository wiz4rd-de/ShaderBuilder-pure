import { beforeEach, describe, expect, it } from "vitest";

import type { LibraryItem } from "../bindings/LibraryItem";
import { nextId, resetIdsForTest } from "../store/ids";
import { instantiateLibraryItem } from "./instantiate";

beforeEach(() => resetIdsForTest());

function nodeItem(): LibraryItem {
  return {
    id: "lib-node",
    name: "N",
    description: null,
    tags: [],
    payload: { kind: "node", node: { id: "orig", kind: "texcoord", position: { x: 1, y: 2 }, data: { a: 1 } } },
  };
}

function subgraphItem(): LibraryItem {
  return {
    id: "lib-sub",
    name: "S",
    description: null,
    tags: [],
    payload: {
      kind: "subgraph",
      subgraph: {
        id: "sub-orig",
        name: "S",
        nodes: [
          { id: "a", kind: "source", position: { x: 0, y: 0 }, data: {} },
          { id: "b", kind: "output", position: { x: 0, y: 0 }, data: {} },
        ],
        edges: [{ id: "e", source: "a", sourcePort: "out", target: "b", targetPort: "color" }],
        boundaryPorts: [
          { name: "coord", ty: "vec2", direction: "in", interiorNode: "a", interiorPort: "coord" },
        ],
      },
    },
  };
}

describe("instantiateLibraryItem", () => {
  it("clones a node payload with a fresh id and deep-cloned data", () => {
    const item = nodeItem();
    const out = instantiateLibraryItem(item, nextId);
    expect(out.kind).toBe("node");
    if (out.kind !== "node") return;
    expect(out.node.id).not.toBe("orig");
    expect(out.node.kind).toBe("texcoord");
    expect(out.node.data).toEqual({ a: 1 });
    // Deep clone: mutating the result does not touch the source.
    (out.node.data as { a: number }).a = 99;
    if (item.payload.kind !== "node") throw new Error("kind");
    expect((item.payload.node.data as { a: number }).a).toBe(1);
  });

  it("rewrites subgraph interior ids, edges, and boundary ports", () => {
    const out = instantiateLibraryItem(subgraphItem(), nextId);
    expect(out.kind).toBe("subgraph");
    if (out.kind !== "subgraph") return;
    const sg = out.subgraph;
    expect(sg.id).not.toBe("sub-orig");
    const [a, b] = sg.nodes;
    expect(a!.id).not.toBe("a");
    expect(b!.id).not.toBe("b");
    // The interior edge endpoints follow the remap.
    expect(sg.edges[0]!.source).toBe(a!.id);
    expect(sg.edges[0]!.target).toBe(b!.id);
    expect(sg.edges[0]!.id).not.toBe("e");
    // Boundary port interiorNode is remapped; the port NAMES stay stable.
    expect(sg.boundaryPorts[0]!.interiorNode).toBe(a!.id);
    expect(sg.boundaryPorts[0]!.name).toBe("coord");
    expect(sg.boundaryPorts[0]!.interiorPort).toBe("coord");
  });

  it("two instantiations of the same item share no ids", () => {
    const item = subgraphItem();
    const first = instantiateLibraryItem(item, nextId);
    const second = instantiateLibraryItem(item, nextId);
    if (first.kind !== "subgraph" || second.kind !== "subgraph") throw new Error("kind");
    const ids = (k: typeof first) =>
      k.kind === "subgraph"
        ? [k.subgraph.id, ...k.subgraph.nodes.map((n) => n.id), ...k.subgraph.edges.map((e) => e.id)]
        : [];
    const a = new Set(ids(first));
    for (const id of ids(second)) {
      expect(a.has(id)).toBe(false);
    }
  });
});
