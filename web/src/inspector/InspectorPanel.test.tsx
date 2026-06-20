import { render, screen, fireEvent, act } from "@testing-library/react";
import { ReactFlowProvider } from "@xyflow/react";
import { beforeEach, describe, expect, it } from "vitest";

import { requireDescriptor } from "../nodes/registry";
import { useDocumentStore } from "../store/documentStore";
import { resetIdsForTest } from "../store/ids";
import { InspectorPanel } from "./InspectorPanel";

function store() {
  return useDocumentStore.getState();
}

/** Add a node of `kind` (with default data) and select it at the pass level. */
function addAndSelect(kind: string, extra: Record<string, unknown> = {}): string {
  const id = store().addNode(kind, { x: 0, y: 0 }, { ...requireDescriptor(kind).defaultData(), ...extra });
  act(() => {
    store().openPass(store().activePassId);
    store().setSelection({ nodeIds: [id], edgeIds: [] });
  });
  return id;
}

function renderPanel() {
  return render(
    <ReactFlowProvider>
      <InspectorPanel />
    </ReactFlowProvider>,
  );
}

function nodeData(id: string): Record<string, unknown> {
  return store().activeGraph().nodes.find((n) => n.id === id)!.data;
}

beforeEach(() => {
  resetIdsForTest();
  store().reset();
});

describe("InspectorPanel — selection states", () => {
  it("shows a placeholder at the pipeline level", () => {
    renderPanel();
    expect(screen.getByText(/Drill into a pass/i)).toBeInTheDocument();
  });

  it("shows a placeholder when nothing is selected in a pass", () => {
    act(() => store().openPass(store().activePassId));
    renderPanel();
    expect(screen.getByText(/Select a node/i)).toBeInTheDocument();
  });

  it("shows a neutral state for a multi-node selection", () => {
    const a = store().addNode("source", { x: 0, y: 0 });
    const b = store().addNode("output", { x: 0, y: 0 });
    act(() => {
      store().openPass(store().activePassId);
      store().setSelection({ nodeIds: [a, b], edgeIds: [] });
    });
    renderPanel();
    expect(screen.getByText(/2 nodes selected/i)).toBeInTheDocument();
  });

  it("renders a selected node's label, id and typed ports", () => {
    const id = addAndSelect("source");
    renderPanel();
    expect(screen.getByText("Source")).toBeInTheDocument();
    expect(screen.getByText(id)).toBeInTheDocument();
    // The read-only coord input port (vec2) is listed.
    expect(screen.getByText("UV")).toBeInTheDocument();
    expect(screen.getAllByText("vec2").length).toBeGreaterThan(0);
  });
});

describe("InspectorPanel — editing writes back into the document", () => {
  it("edits a const value (debounced) into node.data, undoably", () => {
    const id = addAndSelect("const", { constType: "float", value: 0 });
    renderPanel();
    const input = screen.getByDisplayValue("0") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "0.75" } });
    fireEvent.blur(input);
    expect(nodeData(id).value).toBe(0.75);
    expect(store().canUndo()).toBe(true);
    act(() => store().undo());
    expect(nodeData(id).value).toBe(0);
  });

  it("changes a const's type via the select, swapping the value field kind", () => {
    const id = addAndSelect("const", { constType: "float", value: 0 });
    const { container } = renderPanel();
    const select = container.querySelector("select") as HTMLSelectElement;
    fireEvent.change(select, { target: { value: "vec3" } });
    expect(nodeData(id).constType).toBe("vec3");
    // The value widget is now a 3-component vec editor.
    expect(container.querySelectorAll('input[type="number"]')).toHaveLength(3);
  });

  it("edits an indexed sampler's integer index", () => {
    const id = addAndSelect("passOutput", { index: 0 });
    renderPanel();
    const input = screen.getByDisplayValue("0") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "2" } });
    fireEvent.blur(input);
    expect(nodeData(id).index).toBe(2);
  });
});

describe("InspectorPanel — custom snippet (#52)", () => {
  it("shows the code body field and the editable port editor", () => {
    addAndSelect("customSnippet");
    renderPanel();
    expect(screen.getByText("Body (slang)")).toBeInTheDocument();
    // Editable-port controls (rename/retype/add) render for the snippet.
    expect(screen.getByLabelText("Inputs port 0 name")).toBeInTheDocument();
    expect(screen.getByLabelText("Outputs port 0 name")).toBeInTheDocument();
  });

  it("editing the body writes back into node.data", () => {
    const id = addAndSelect("customSnippet");
    renderPanel();
    const body = screen.getByLabelText("Body (slang)") as HTMLTextAreaElement;
    fireEvent.change(body, { target: { value: "result = color;" } });
    fireEvent.blur(body);
    expect(nodeData(id).body).toBe("result = color;");
  });

  it("renaming an input port keeps it editable + lowers with the new name", () => {
    const id = addAndSelect("customSnippet");
    renderPanel();
    fireEvent.change(screen.getByLabelText("Inputs port 0 name"), {
      target: { value: "rgb" },
    });
    const inputs = nodeData(id).inputs as { name: string }[];
    expect(inputs[0]!.name).toBe("rgb");
  });

  it("surfaces a pre-check warning for a port the body never references", () => {
    // Default body references `color`/`result`; rename the input so it no longer
    // appears in the body — the cheap pre-check flags it (compile_graph is #54).
    addAndSelect("customSnippet", {
      body: "result = vec4(1.0);",
      inputs: [{ name: "color", type: "vec4" }],
      outputs: [{ name: "result", type: "vec4" }],
    });
    renderPanel();
    expect(screen.getByText(/declared but never referenced/i)).toBeInTheDocument();
  });
});

describe("InspectorPanel — diagnostics", () => {
  it("renders diagnostics keyed by the node id", () => {
    const id = addAndSelect("source");
    act(() =>
      store().setDiagnosticsByNode({
        [id]: [
          { severity: "error", code: "danglingInput", message: "coord unconnected", node: id, port: "coord" },
        ],
      }),
    );
    renderPanel();
    expect(screen.getByText("danglingInput")).toBeInTheDocument();
    expect(screen.getByText("coord unconnected")).toBeInTheDocument();
  });
});
