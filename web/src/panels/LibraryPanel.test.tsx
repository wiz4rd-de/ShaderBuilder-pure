import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { ReactFlowProvider } from "@xyflow/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { LibraryItem } from "../bindings/LibraryItem";

// One shared invoke mock; per-test we set its implementation to fake the three
// library commands (the mocked-invoke vitest pattern, mirrored from #55's tests).
const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import { instantiateLibraryItem } from "../library/instantiate";
import { isSubgraphNode, readSubgraph } from "../nodes/subgraph";
import { useDocumentStore } from "../store/documentStore";
import { nextId, resetIdsForTest } from "../store/ids";
import { LibraryPanel } from "./LibraryPanel";

function store() {
  return useDocumentStore.getState();
}

function renderPanel() {
  return render(
    <ReactFlowProvider>
      <LibraryPanel />
    </ReactFlowProvider>,
  );
}

/** Drill into the default pass so the canvas is at the "pass" level. */
function openPass(): void {
  store().openPass(store().activePassId);
}

/** A single-node library item (texcoord). */
function nodeItem(): LibraryItem {
  return {
    id: "lib-node",
    name: "My Coord",
    description: "a coord",
    tags: ["uv"],
    payload: {
      kind: "node",
      node: { id: "n-orig", kind: "texcoord", position: { x: 0, y: 0 }, data: {} },
    },
  };
}

/** A subgraph library item: one interior node with one in + one out boundary. */
function subgraphItem(): LibraryItem {
  return {
    id: "lib-sub",
    name: "My Sub",
    description: null,
    tags: [],
    payload: {
      kind: "subgraph",
      subgraph: {
        id: "sub-orig",
        name: "My Sub",
        nodes: [{ id: "s-orig", kind: "source", position: { x: 0, y: 0 }, data: {} }],
        edges: [],
        boundaryPorts: [
          { name: "coord", ty: "vec2", direction: "in", interiorNode: "s-orig", interiorPort: "coord" },
          { name: "out", ty: "vec4", direction: "out", interiorNode: "s-orig", interiorPort: "out" },
        ],
      },
    },
  };
}

beforeEach(() => {
  resetIdsForTest();
  store().reset();
  invoke.mockReset();
});

describe("LibraryPanel", () => {
  it("lists persisted items from list_library_node on mount", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_library_node") return Promise.resolve([nodeItem(), subgraphItem()]);
      return Promise.resolve();
    });
    renderPanel();
    await waitFor(() => expect(screen.getByText("My Coord")).toBeInTheDocument());
    expect(screen.getByText("My Sub")).toBeInTheDocument();
    // Kinds are surfaced (single node vs subgraph).
    expect(screen.getByText("node")).toBeInTheDocument();
    expect(screen.getByText("subgraph")).toBeInTheDocument();
    // Tags + description render.
    expect(screen.getByText("uv")).toBeInTheDocument();
    expect(screen.getByText("a coord")).toBeInTheDocument();
  });

  it("saves a single selected node as a node-payload LibraryItem", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_library_node") return Promise.resolve([]);
      return Promise.resolve();
    });
    openPass();
    const nodeId = store().addNode("texcoord", { x: 10, y: 20 });
    store().setSelection({ nodeIds: [nodeId], edgeIds: [] });

    const promptSpy = vi
      .spyOn(window, "prompt")
      .mockReturnValueOnce("Saved Coord") // name
      .mockReturnValueOnce("") // description
      .mockReturnValueOnce("tagA, tagB"); // tags

    renderPanel();
    fireEvent.click(screen.getByRole("button", { name: "Save selection" }));

    await waitFor(() => {
      const call = invoke.mock.calls.find((c) => c[0] === "save_library_node");
      expect(call).toBeTruthy();
    });
    const saveCall = invoke.mock.calls.find((c) => c[0] === "save_library_node")!;
    const item = (saveCall[1] as { item: LibraryItem }).item;
    expect(item.name).toBe("Saved Coord");
    expect(item.tags).toEqual(["tagA", "tagB"]);
    expect(item.payload.kind).toBe("node");
    if (item.payload.kind === "node") {
      expect(item.payload.node.kind).toBe("texcoord");
    }
    promptSpy.mockRestore();
  });

  it("inserts a single-node item into the active graph with a fresh id", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_library_node") return Promise.resolve([nodeItem()]);
      return Promise.resolve();
    });
    openPass();
    renderPanel();
    await waitFor(() => expect(screen.getByText("My Coord")).toBeInTheDocument());

    fireEvent.click(screen.getByRole("button", { name: "Insert" }));
    const g1 = store().activeGraph();
    expect(g1.nodes).toHaveLength(1);
    const first = g1.nodes[0]!;
    expect(first.kind).toBe("texcoord");
    expect(first.id).not.toBe("n-orig"); // fresh id, not the library body's id

    // A second insert of the SAME item yields a non-clashing id.
    fireEvent.click(screen.getByRole("button", { name: "Insert" }));
    const g2 = store().activeGraph();
    expect(g2.nodes).toHaveLength(2);
    expect(g2.nodes[0]!.id).not.toBe(g2.nodes[1]!.id);
  });

  it("inserts a subgraph item as a collapsed, drill-in-editable node with fresh ids", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_library_node") return Promise.resolve([subgraphItem()]);
      return Promise.resolve();
    });
    openPass();
    renderPanel();
    await waitFor(() => expect(screen.getByText("My Sub")).toBeInTheDocument());

    fireEvent.click(screen.getByRole("button", { name: "Insert" }));
    const g1 = store().activeGraph();
    expect(g1.nodes).toHaveLength(1);
    const wrapper1 = g1.nodes[0]!;
    expect(isSubgraphNode(wrapper1)).toBe(true);
    const sub1 = readSubgraph(wrapper1);
    expect(sub1.id).not.toBe("sub-orig"); // fresh body id
    expect(sub1.nodes[0]!.id).not.toBe("s-orig"); // fresh interior id
    // Boundary ports were remapped onto the fresh interior node id.
    expect(sub1.boundaryPorts.every((b) => b.interiorNode === sub1.nodes[0]!.id)).toBe(true);

    // A second insert: no shared ids between the two subgraph bodies/interiors.
    fireEvent.click(screen.getByRole("button", { name: "Insert" }));
    const g2 = store().activeGraph();
    expect(g2.nodes).toHaveLength(2);
    const sub2 = readSubgraph(g2.nodes[1]!);
    expect(g2.nodes[0]!.id).not.toBe(g2.nodes[1]!.id);
    expect(sub1.id).not.toBe(sub2.id);
    expect(sub1.nodes[0]!.id).not.toBe(sub2.nodes[0]!.id);
  });

  it("mints a durable-unique item id per save (survives a counter reset across sessions)", async () => {
    // Fix B: the persisted item id must be durable-unique. `nextId` resets to 0
    // each launch, so two sessions' first saves would both mint `lib-1` and the
    // backend would overwrite the prior item. `crypto.randomUUID` is durable.
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_library_node") return Promise.resolve([]);
      return Promise.resolve();
    });
    const promptSpy = vi
      .spyOn(window, "prompt")
      .mockReturnValue("Name") // every prompt (name/description/tags)
      .mockReturnValueOnce("First");

    // --- session 1 ---
    openPass();
    const n1 = store().addNode("texcoord", { x: 0, y: 0 });
    store().setSelection({ nodeIds: [n1], edgeIds: [] });
    const { unmount } = renderPanel();
    fireEvent.click(screen.getByRole("button", { name: "Save selection" }));
    await waitFor(() =>
      expect(invoke.mock.calls.some((c) => c[0] === "save_library_node")).toBe(true),
    );
    const firstId = (invoke.mock.calls.find((c) => c[0] === "save_library_node")![1] as {
      item: LibraryItem;
    }).item.id;
    unmount();

    // --- session 2: a fresh launch resets the per-process id counter to 0 ---
    invoke.mockClear();
    resetIdsForTest();
    store().reset();
    openPass();
    const n2 = store().addNode("texcoord", { x: 0, y: 0 });
    store().setSelection({ nodeIds: [n2], edgeIds: [] });
    renderPanel();
    fireEvent.click(screen.getByRole("button", { name: "Save selection" }));
    await waitFor(() =>
      expect(invoke.mock.calls.some((c) => c[0] === "save_library_node")).toBe(true),
    );
    const secondId = (invoke.mock.calls.find((c) => c[0] === "save_library_node")![1] as {
      item: LibraryItem;
    }).item.id;

    // Distinct ids despite the counter reset — no cross-session overwrite.
    expect(secondId).not.toBe(firstId);
    promptSpy.mockRestore();
  });

  it("multi-selection save uses the NEW collapse wrapper, not a pre-existing subgraph", async () => {
    // Fix D: a graph already containing an UNSELECTED subgraph node A (appears
    // earlier) plus primitives B+C. Selecting only B+C and saving must persist the
    // B+C subgraph (the new wrapper, appended last), NOT A's body.
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_library_node") return Promise.resolve([]);
      return Promise.resolve();
    });
    openPass();
    // A: a pre-existing collapsed subgraph (collapse a lone node so it sits first).
    const a0 = store().addNode("source", { x: -100, y: 0 });
    store().setSelection({ nodeIds: [a0], edgeIds: [] });
    store().collapseSelection("PreExisting A");
    const aId = store().activeGraph().nodes.find(isSubgraphNode)!.id;
    // B + C: two primitives wired together, both selected.
    const b = store().addNode("texcoord", { x: 0, y: 0 });
    const c = store().addNode("source", { x: 100, y: 0 });
    store().addEdge(b, "uv", c, "coord");
    store().setSelection({ nodeIds: [b, c], edgeIds: [] });

    const promptSpy = vi
      .spyOn(window, "prompt")
      .mockReturnValueOnce("BC") // name
      .mockReturnValueOnce("") // description
      .mockReturnValueOnce(""); // tags

    renderPanel();
    fireEvent.click(screen.getByRole("button", { name: "Save selection" }));
    await waitFor(() =>
      expect(invoke.mock.calls.some((call) => call[0] === "save_library_node")).toBe(true),
    );
    const item = (invoke.mock.calls.find((call) => call[0] === "save_library_node")![1] as {
      item: LibraryItem;
    }).item;
    expect(item.payload.kind).toBe("subgraph");
    if (item.payload.kind !== "subgraph") throw new Error("kind");
    const sub = item.payload.subgraph;
    // The saved body is B+C (a texcoord + a source), NOT A's lone source.
    expect(sub.nodes.map((n) => n.kind).sort()).toEqual(["source", "texcoord"]);
    // A's id is nowhere in the saved interior.
    expect(sub.nodes.some((n) => n.id === aId)).toBe(false);
    promptSpy.mockRestore();
  });

  it("single selected subgraph node is saved as a subgraph payload (re-id'd on insert)", async () => {
    // Fix E: a single collapsed subgraph node saved as a `node` payload would NOT
    // re-id its interior on insert, so two inserts collide. Saving it as a
    // `subgraph` payload lets instantiate re-mint the whole interior.
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_library_node") return Promise.resolve([]);
      return Promise.resolve();
    });
    openPass();
    const src = store().addNode("source", { x: 0, y: 0 });
    store().setSelection({ nodeIds: [src], edgeIds: [] });
    store().collapseSelection("Solo");
    const sgId = store().activeGraph().nodes.find(isSubgraphNode)!.id;
    store().setSelection({ nodeIds: [sgId], edgeIds: [] });

    const promptSpy = vi
      .spyOn(window, "prompt")
      .mockReturnValueOnce("Solo") // name
      .mockReturnValueOnce("") // description
      .mockReturnValueOnce(""); // tags

    renderPanel();
    fireEvent.click(screen.getByRole("button", { name: "Save selection" }));
    await waitFor(() =>
      expect(invoke.mock.calls.some((call) => call[0] === "save_library_node")).toBe(true),
    );
    const item = (invoke.mock.calls.find((call) => call[0] === "save_library_node")![1] as {
      item: LibraryItem;
    }).item;
    expect(item.payload.kind).toBe("subgraph");

    // Two instantiations of the saved item share NO ids (interior node/edge ids +
    // Subgraph.id all distinct) — the freshness invariant the subgraph path enforces.
    const first = instantiateLibraryItem(item, nextId);
    const second = instantiateLibraryItem(item, nextId);
    if (first.kind !== "subgraph" || second.kind !== "subgraph") throw new Error("kind");
    const idsOf = (p: typeof first) =>
      p.kind === "subgraph"
        ? [p.subgraph.id, ...p.subgraph.nodes.map((n) => n.id), ...p.subgraph.edges.map((e) => e.id)]
        : [];
    const firstIds = new Set(idsOf(first));
    for (const id of idsOf(second)) {
      expect(firstIds.has(id)).toBe(false);
    }
    promptSpy.mockRestore();
  });

  it("deletes an item behind a confirm and refreshes", async () => {
    let listed: LibraryItem[] = [nodeItem()];
    invoke.mockImplementation((cmd: string, args?: unknown) => {
      if (cmd === "list_library_node") return Promise.resolve(listed);
      if (cmd === "delete_library_node") {
        const id = (args as { id: string }).id;
        listed = listed.filter((i) => i.id !== id);
        return Promise.resolve();
      }
      return Promise.resolve();
    });
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    renderPanel();
    await waitFor(() => expect(screen.getByText("My Coord")).toBeInTheDocument());

    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    await waitFor(() => {
      expect(invoke.mock.calls.some((c) => c[0] === "delete_library_node")).toBe(true);
    });
    await waitFor(() => expect(screen.queryByText("My Coord")).toBeNull());
    confirmSpy.mockRestore();
  });
});
