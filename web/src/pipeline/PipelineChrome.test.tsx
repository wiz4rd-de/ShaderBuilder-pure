import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";

import { EditorStatusBar } from "../editor/EditorStatusBar";
import { useDocumentStore } from "../store/documentStore";
import { resetIdsForTest } from "../store/ids";
import { PipelineBreadcrumb } from "./PipelineBreadcrumb";
import { PipelineToolbar } from "./PipelineToolbar";

// The pipeline chrome (breadcrumb + toolbar + status bar) drives the shared
// document store. The React Flow canvas itself can't be measured in jsdom, so we
// exercise the user-facing controls and assert the resulting store state.
function Chrome() {
  return (
    <>
      <PipelineBreadcrumb />
      <PipelineToolbar />
      <EditorStatusBar />
    </>
  );
}

function selectPass(passId: string) {
  act(() => useDocumentStore.getState().setPipelineSelection(passId));
}

beforeEach(() => {
  resetIdsForTest();
  useDocumentStore.getState().reset();
});

describe("Pipeline chrome", () => {
  it("Add pass appends a pass and the status bar reflects the count", async () => {
    const user = userEvent.setup();
    render(<Chrome />);
    expect(screen.getByTestId("status-counts")).toHaveTextContent("1 pass");

    await user.click(screen.getByRole("button", { name: "Add pass" }));
    expect(screen.getByTestId("status-counts")).toHaveTextContent("2 passes");
    expect(useDocumentStore.getState().project.passes).toHaveLength(2);
  });

  it("Move left/right reorders the selected pass", async () => {
    const user = userEvent.setup();
    render(<Chrome />);
    await user.click(screen.getByRole("button", { name: "Add pass" }));
    const [p0, p1] = useDocumentStore.getState().project.passes.map((p) => p.id);

    // Select the second pass and move it left → it becomes index 0.
    selectPass(p1!);
    await user.click(screen.getByRole("button", { name: "Move left" }));
    expect(useDocumentStore.getState().project.passes.map((p) => p.id)).toEqual([p1, p0]);
  });

  it("Remove pass deletes the selected pass (and not the last one)", async () => {
    const user = userEvent.setup();
    render(<Chrome />);
    await user.click(screen.getByRole("button", { name: "Add pass" }));
    const p1 = useDocumentStore.getState().project.passes[1]!.id;

    selectPass(p1);
    await user.click(screen.getByRole("button", { name: "Remove pass" }));
    expect(useDocumentStore.getState().project.passes).toHaveLength(1);

    // With a single pass left, Remove is disabled.
    selectPass(useDocumentStore.getState().project.passes[0]!.id);
    expect(screen.getByRole("button", { name: "Remove pass" })).toBeDisabled();
  });

  it("Open pass drills in; the breadcrumb back returns to the pipeline", async () => {
    const user = userEvent.setup();
    render(<Chrome />);
    const p0 = useDocumentStore.getState().project.passes[0]!.id;

    selectPass(p0);
    await user.click(screen.getByRole("button", { name: "Open pass" }));
    expect(useDocumentStore.getState().level).toBe("pass");
    expect(screen.getByTestId("breadcrumb-pass")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Back to pipeline" }));
    expect(useDocumentStore.getState().level).toBe("pipeline");
  });

  it("drilling into a subgraph shows its crumb; back pops one level (#57)", async () => {
    const user = userEvent.setup();
    render(<Chrome />);
    const store = useDocumentStore.getState();
    const p0 = store.project.passes[0]!.id;

    // Build a node in the pass, collapse it into a named subgraph, drill in.
    store.openPass(p0);
    const n = store.addNode("source", { x: 0, y: 0 });
    act(() => store.setSelection({ nodeIds: [n], edgeIds: [] }));
    act(() => store.collapseSelection("Grouped"));
    const sgId = useDocumentStore
      .getState()
      .activeGraph()
      .nodes.find((node) => node.kind === "subgraph")!.id;
    act(() => useDocumentStore.getState().openSubgraph(sgId));

    // The breadcrumb shows the subgraph name as the current crumb.
    const crumb = screen.getByTestId("breadcrumb-subgraph");
    expect(crumb).toHaveTextContent("Grouped");
    expect(crumb).toHaveAttribute("data-subgraph-id", sgId);

    // Back pops one level — to the pass graph (still level "pass", path empty).
    await user.click(screen.getByRole("button", { name: `Back to ${"Pass 1"}` }));
    const after = useDocumentStore.getState();
    expect(after.level).toBe("pass");
    expect(after.subgraphPath).toEqual([]);
  });
});
