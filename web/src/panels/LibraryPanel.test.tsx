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

import { isSubgraphNode, readSubgraph } from "../nodes/subgraph";
import { useDocumentStore } from "../store/documentStore";
import { resetIdsForTest } from "../store/ids";
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
