import { fireEvent, render, screen } from "@testing-library/react";
import { ReactFlowProvider } from "@xyflow/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(() => Promise.resolve()) }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(vi.fn())),
}));

import { useDocumentStore } from "../store/documentStore";
import { resetIdsForTest } from "../store/ids";
import { PanelLayout } from "./PanelLayout";

function renderLayout() {
  return render(
    <ReactFlowProvider>
      <PanelLayout />
    </ReactFlowProvider>,
  );
}

beforeEach(() => {
  resetIdsForTest();
  useDocumentStore.getState().reset();
});

describe("PanelLayout", () => {
  it("shows the inspector tab by default and switches tabs", () => {
    renderLayout();
    expect(screen.getByRole("tab", { name: "Inspector" })).toHaveAttribute(
      "aria-selected",
      "true",
    );

    fireEvent.click(screen.getByRole("tab", { name: "Pass" }));
    expect(screen.getByLabelText("Pass settings")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("tab", { name: "Viewport" }));
    expect(screen.getByLabelText("Viewport")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("tab", { name: "Source" }));
    expect(screen.getByLabelText("Source")).toBeInTheDocument();
  });
});
