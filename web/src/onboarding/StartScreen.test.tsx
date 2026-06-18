import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

// Mock the start actions + recents/help seams so the screen renders with no Tauri
// runtime and we can assert the button → action wiring.
const actions = {
  startNew: vi.fn(async () => undefined),
  startOpen: vi.fn(async () => undefined),
  startExample: vi.fn(async () => undefined),
  startImport: vi.fn(async () => undefined),
};
vi.mock("./startActions", () => ({
  startNew: () => actions.startNew(),
  startOpen: () => actions.startOpen(),
  startExample: () => actions.startExample(),
  startImport: () => actions.startImport(),
}));

vi.mock("./useRecents", () => ({
  useRecents: () => [],
}));

import { StartScreen } from "./StartScreen";

beforeEach(() => {
  vi.clearAllMocks();
});

describe("StartScreen", () => {
  it("renders the four starting actions and a help link", () => {
    render(<StartScreen />);
    expect(screen.getByRole("button", { name: /New project/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Open example/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Open project/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Import preset/ })).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /Help & keyboard shortcuts/ }),
    ).toBeInTheDocument();
  });

  it("Open example invokes the example action", () => {
    render(<StartScreen />);
    fireEvent.click(screen.getByRole("button", { name: /Open example/ }));
    expect(actions.startExample).toHaveBeenCalledOnce();
  });

  it("New / Open / Import wire to their actions", () => {
    render(<StartScreen />);
    fireEvent.click(screen.getByRole("button", { name: /New project/ }));
    fireEvent.click(screen.getByRole("button", { name: /Open project/ }));
    fireEvent.click(screen.getByRole("button", { name: /Import preset/ }));
    expect(actions.startNew).toHaveBeenCalledOnce();
    expect(actions.startOpen).toHaveBeenCalledOnce();
    expect(actions.startImport).toHaveBeenCalledOnce();
  });

  it("opens the help dialog from the footer link", () => {
    render(<StartScreen />);
    expect(screen.queryByRole("dialog", { name: "Help" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Help & keyboard shortcuts/ }));
    expect(screen.getByRole("dialog", { name: "Help" })).toBeInTheDocument();
  });
});
