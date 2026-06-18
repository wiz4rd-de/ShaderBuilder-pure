import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";

import { useDocumentStore } from "../store/documentStore";
import { resetIdsForTest } from "../store/ids";
import { EditorStatusBar } from "./EditorStatusBar";
import { EditorToolbar } from "./EditorToolbar";

// Component-level coverage of the editor chrome: toolbar buttons + status bar
// read and drive the shared document store. React Flow itself is exercised
// headlessly in the store tests (jsdom can't measure the canvas), so here we
// verify the user-facing controls wire through to store state.
function Harness() {
  return (
    <>
      <EditorToolbar />
      <EditorStatusBar />
    </>
  );
}

beforeEach(() => {
  resetIdsForTest();
  useDocumentStore.getState().reset();
  // The toolbar/status here exercise the per-pass graph level; drill into the
  // initial pass so the status bar reports node/edge counts (not pass counts).
  const firstPass = useDocumentStore.getState().project.passes[0]!.id;
  useDocumentStore.getState().openPass(firstPass);
});

describe("EditorToolbar + EditorStatusBar", () => {
  it("Add node inserts a node and the status bar reflects the count", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    expect(screen.getByTestId("status-counts")).toHaveTextContent("0 nodes");

    await user.click(screen.getByRole("button", { name: "Add node" }));
    expect(screen.getByTestId("status-counts")).toHaveTextContent("1 node,");
    expect(screen.getByTestId("status-dirty")).toHaveTextContent("Unsaved changes");
  });

  it("Undo/Redo buttons enable with history and reverse an add", async () => {
    const user = userEvent.setup();
    render(<Harness />);

    const undo = screen.getByRole("button", { name: "Undo" });
    const redo = screen.getByRole("button", { name: "Redo" });
    expect(undo).toBeDisabled();
    expect(redo).toBeDisabled();

    await user.click(screen.getByRole("button", { name: "Add node" }));
    expect(undo).toBeEnabled();

    await user.click(undo);
    expect(screen.getByTestId("status-counts")).toHaveTextContent("0 nodes");
    expect(redo).toBeEnabled();

    await user.click(redo);
    expect(screen.getByTestId("status-counts")).toHaveTextContent("1 node,");
  });

  it("Copy/Paste/Duplicate/Delete reflect selection in the status bar", async () => {
    const user = userEvent.setup();
    render(<Harness />);

    // Add two nodes, select one via the store (selection normally comes from RF).
    await user.click(screen.getByRole("button", { name: "Add node" }));
    await user.click(screen.getByRole("button", { name: "Add node" }));
    const firstId = useDocumentStore.getState().activeGraph().nodes[0]!.id;
    act(() => {
      useDocumentStore.getState().setSelection({ nodeIds: [firstId], edgeIds: [] });
    });

    expect(screen.getByTestId("status-selection")).toHaveTextContent("1 selected");

    // Copy enables paste; paste adds a node.
    await user.click(screen.getByRole("button", { name: "Copy" }));
    await user.click(screen.getByRole("button", { name: "Paste" }));
    expect(screen.getByTestId("status-counts")).toHaveTextContent("3 nodes");

    // Delete removes the currently-selected (pasted) node.
    await user.click(screen.getByRole("button", { name: "Delete" }));
    expect(screen.getByTestId("status-counts")).toHaveTextContent("2 nodes");
  });
});
