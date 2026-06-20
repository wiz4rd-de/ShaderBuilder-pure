import { ReactFlowProvider, type NodeProps } from "@xyflow/react";
import { render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { useDocumentStore } from "../store/documentStore";
import { requireDescriptor } from "./registry";
import { TaxonomyNode } from "./TaxonomyNode";

// Minimal NodeProps for a controlled render. React Flow's <Handle> needs the
// provider context (the store) to mount; the rest of NodeProps is unused by
// TaxonomyNode, so we cast a partial through unknown.
function nodeProps(kind: string, data: Record<string, unknown> = {}): NodeProps {
  return {
    id: `${kind}-1`,
    type: kind,
    data: { ...requireDescriptor(kind).defaultData(), ...data },
    selected: false,
    dragging: false,
    zIndex: 0,
    isConnectable: true,
    positionAbsoluteX: 0,
    positionAbsoluteY: 0,
    deletable: true,
    selectable: true,
    draggable: true,
  } as unknown as NodeProps;
}

function renderNode(kind: string, data: Record<string, unknown> = {}) {
  return render(
    <ReactFlowProvider>
      <TaxonomyNode {...nodeProps(kind, data)} />
    </ReactFlowProvider>,
  );
}

afterEach(() => {
  // Clear diagnostics so badge tests don't bleed into the others.
  useDocumentStore.getState().setDiagnosticsByNode({});
});

describe("TaxonomyNode", () => {
  it("renders a sampler's title + its coord input and out output handles", () => {
    const { container } = renderNode("source");
    expect(screen.getByText("Source")).toBeInTheDocument();
    // One target handle for "coord", one source handle for "out".
    const targets = container.querySelectorAll(".react-flow__handle-left");
    const sources = container.querySelectorAll(".react-flow__handle-right");
    expect(targets).toHaveLength(1);
    expect(sources).toHaveLength(1);
    // Handles carry the declared port ids.
    expect(container.querySelector('[data-handleid="coord"]')).not.toBeNull();
    expect(container.querySelector('[data-handleid="out"]')).not.toBeNull();
  });

  it("reflects a const node's data-dependent output type on the handle", () => {
    const { container } = renderNode("const", { constType: "vec3", value: [0, 0, 0] });
    // The out handle gets the vec3 type class from the descriptor's outputs(data).
    expect(container.querySelector(".taxonomy-node__handle--vec3")).not.toBeNull();
  });

  it("renders the output node with only an input handle", () => {
    const { container } = renderNode("output");
    expect(screen.getByText("Output")).toBeInTheDocument();
    expect(container.querySelectorAll(".react-flow__handle-left")).toHaveLength(1);
    expect(container.querySelectorAll(".react-flow__handle-right")).toHaveLength(0);
  });

  it("shows an inline error badge when the node has a compile diagnostic (#54)", () => {
    useDocumentStore.getState().setDiagnosticsByNode({
      "source-1": [
        { severity: "error", code: "danglingInput", message: "coord unconnected", node: "source-1", port: "coord" },
      ],
    });
    const { container } = renderNode("source");
    const badge = container.querySelector(".taxonomy-node__badge--error");
    expect(badge).not.toBeNull();
    expect(container.querySelector(".taxonomy-node--error")).not.toBeNull();
  });

  it("shows a warning badge (not error) for a warning-only diagnostic", () => {
    useDocumentStore.getState().setDiagnosticsByNode({
      "source-1": [
        { severity: "warning", code: "unusedPort", message: "meh", node: "source-1", port: null },
      ],
    });
    const { container } = renderNode("source");
    expect(container.querySelector(".taxonomy-node__badge--warning")).not.toBeNull();
    expect(container.querySelector(".taxonomy-node__badge--error")).toBeNull();
  });

  it("falls back to an 'unknown node' card for an unregistered kind", () => {
    const { container } = render(
      <ReactFlowProvider>
        <TaxonomyNode {...nodeProps("source")} type="mystery-kind" />
      </ReactFlowProvider>,
    );
    expect(screen.getByText("mystery-kind")).toBeInTheDocument();
    expect(container.querySelector(".taxonomy-node--unknown")).not.toBeNull();
  });
});
