import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { RecentProject } from "../bindings/RecentProject";

// Mock the IO/dialog seams so the menu renders with no Tauri runtime.
const loadRecents = vi.fn<() => Promise<RecentProject[]>>();
vi.mock("./api", () => ({
  loadRecents: () => loadRecents(),
}));

const actions = {
  newProject: vi.fn(async () => undefined),
  openProject: vi.fn(async () => undefined),
  openRecent: vi.fn(async (_path: string) => undefined),
  save: vi.fn(async () => true),
  saveAs: vi.fn(async () => true),
};
vi.mock("./projectActions", () => ({
  newProject: () => actions.newProject(),
  openProject: () => actions.openProject(),
  openRecent: (p: string) => actions.openRecent(p),
  save: () => actions.save(),
  saveAs: () => actions.saveAs(),
}));

import { FileMenu } from "./FileMenu";

beforeEach(() => {
  vi.clearAllMocks();
  loadRecents.mockResolvedValue([]);
});

describe("FileMenu", () => {
  it("opens the dropdown and shows the standard items", () => {
    render(<FileMenu />);
    fireEvent.click(screen.getByRole("button", { name: "File" }));
    expect(screen.getByRole("menuitem", { name: /New/ })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /Open…/ })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "Save Ctrl+S" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /Save As…/ })).toBeInTheDocument();
  });

  it("each item dispatches its flow", () => {
    render(<FileMenu />);
    fireEvent.click(screen.getByRole("button", { name: "File" }));
    fireEvent.click(screen.getByRole("menuitem", { name: /New/ }));
    expect(actions.newProject).toHaveBeenCalledOnce();

    fireEvent.click(screen.getByRole("button", { name: "File" }));
    fireEvent.click(screen.getByRole("menuitem", { name: /Save As…/ }));
    expect(actions.saveAs).toHaveBeenCalledOnce();
  });

  it("shows recents and opening one routes to openRecent", async () => {
    loadRecents.mockResolvedValue([
      { path: "/home/me/cool.json", name: "Cool Shader" },
    ]);
    render(<FileMenu />);
    fireEvent.click(screen.getByRole("button", { name: "File" }));

    const entry = await screen.findByRole("menuitem", { name: /Cool Shader/ });
    fireEvent.click(entry);
    expect(actions.openRecent).toHaveBeenCalledWith("/home/me/cool.json");
  });

  it("shows an empty-state when there are no recents", async () => {
    loadRecents.mockResolvedValue([]);
    render(<FileMenu />);
    fireEvent.click(screen.getByRole("button", { name: "File" }));
    await waitFor(() => {
      expect(screen.getByText("No recent projects")).toBeInTheDocument();
    });
  });
});
