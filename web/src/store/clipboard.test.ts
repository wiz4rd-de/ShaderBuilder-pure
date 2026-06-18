import { beforeEach, describe, expect, it } from "vitest";

import type { Edge } from "../bindings/Edge";
import type { Node } from "../bindings/Node";
import { captureClipboard, instantiateClipboard } from "./clipboard";
import { resetIdsForTest } from "./ids";

function node(id: string, x = 0, y = 0): Node {
  return { id, kind: "placeholder", position: { x, y }, data: { tag: id } };
}

function edge(id: string, source: string, target: string): Edge {
  return { id, source, sourcePort: "out", target, targetPort: "in" };
}

describe("clipboard", () => {
  beforeEach(() => resetIdsForTest());

  it("captures only edges fully internal to the selection", () => {
    const nodes = [node("a"), node("b"), node("c")];
    const edges = [
      edge("e1", "a", "b"), // internal to {a,b}
      edge("e2", "b", "c"), // dangles outside {a,b}
    ];
    const clip = captureClipboard(nodes, edges, ["a", "b"]);
    expect(clip.nodes.map((n) => n.id).sort()).toEqual(["a", "b"]);
    expect(clip.edges.map((e) => e.id)).toEqual(["e1"]);
  });

  it("deep-clones captured nodes (no shared references)", () => {
    const nodes = [node("a")];
    const clip = captureClipboard(nodes, [], ["a"]);
    clip.nodes[0]!.position.x = 999;
    (clip.nodes[0]!.data as Record<string, unknown>)["tag"] = "mutated";
    expect(nodes[0]!.position.x).toBe(0);
    expect(nodes[0]!.data["tag"]).toBe("a");
  });

  it("instantiates with FRESH node ids and re-points internal edges", () => {
    const nodes = [node("a", 10, 20), node("b", 30, 40)];
    const edges = [edge("e1", "a", "b")];
    const clip = captureClipboard(nodes, edges, ["a", "b"]);

    const fresh = instantiateClipboard(clip, { x: 5, y: 7 });

    // All node ids are new (none collide with the originals).
    const newIds = fresh.nodes.map((n) => n.id);
    expect(newIds).not.toContain("a");
    expect(newIds).not.toContain("b");
    expect(new Set(newIds).size).toBe(2);

    // Positions are offset.
    const byTag = new Map(fresh.nodes.map((n) => [n.data["tag"], n] as const));
    expect(byTag.get("a")!.position).toEqual({ x: 15, y: 27 });
    expect(byTag.get("b")!.position).toEqual({ x: 35, y: 47 });

    // The edge is re-pointed onto the NEW ids, with a fresh edge id, and the
    // ports are preserved.
    expect(fresh.edges).toHaveLength(1);
    const e = fresh.edges[0]!;
    expect(e.id).not.toBe("e1");
    expect(e.source).toBe(byTag.get("a")!.id);
    expect(e.target).toBe(byTag.get("b")!.id);
    expect(e.sourcePort).toBe("out");
    expect(e.targetPort).toBe("in");
  });

  it("two pastes of the same clipboard produce disjoint id sets", () => {
    const clip = captureClipboard([node("a"), node("b")], [edge("e1", "a", "b")], [
      "a",
      "b",
    ]);
    const first = instantiateClipboard(clip, { x: 5, y: 5 });
    const second = instantiateClipboard(clip, { x: 10, y: 10 });
    const firstIds = new Set([...first.nodes, ...first.edges].map((x) => x.id));
    const secondIds = [...second.nodes, ...second.edges].map((x) => x.id);
    for (const id of secondIds) {
      expect(firstIds.has(id)).toBe(false);
    }
  });
});
