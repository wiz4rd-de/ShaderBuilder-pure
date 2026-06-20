import { fireEvent, render, screen, waitFor } from "@testing-library/react";
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

// Recents are mocked per-test via this mutable holder (default: none).
const recentsHolder: { value: { path: string; name: string }[] } = { value: [] };
vi.mock("./useRecents", () => ({
  useRecents: () => recentsHolder.value,
}));

// openRecent is the session seam Open-Recent calls; the F19 test drives it.
const openRecentMock = vi.fn(async (_path: string) => undefined);
vi.mock("../session/projectActions", () => ({
  openRecent: (path: string) => openRecentMock(path),
}));

import { StartScreen } from "./StartScreen";
import { useDocumentStore } from "../store/documentStore";
import { useOnboardingStore } from "./onboardingStore";

beforeEach(() => {
  vi.clearAllMocks();
  recentsHolder.value = [];
  openRecentMock.mockImplementation(async () => undefined);
  useDocumentStore.getState().reset();
  useOnboardingStore.setState({ started: false });
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

  describe("Open-Recent (F19)", () => {
    beforeEach(() => {
      recentsHolder.value = [{ path: "/tmp/recent.json", name: "Recent" }];
    });

    it("enters the editor when the recent file loads", async () => {
      // A successful load sets currentProjectPath; the start screen dismisses.
      openRecentMock.mockImplementation(async () => {
        useDocumentStore.setState({ currentProjectPath: "/tmp/recent.json" });
      });
      render(<StartScreen />);
      fireEvent.click(screen.getByRole("button", { name: /Recent/ }));
      await waitFor(() =>
        expect(useOnboardingStore.getState().started).toBe(true),
      );
    });

    it("stays on the start screen when the recent file fails to load", async () => {
      // openRecent toasts + changes nothing (path stays null) on a missing file:
      // the start screen must NOT dismiss onto a stale empty editor.
      openRecentMock.mockImplementation(async () => undefined);
      render(<StartScreen />);
      fireEvent.click(screen.getByRole("button", { name: /Recent/ }));
      await waitFor(() => expect(openRecentMock).toHaveBeenCalledWith("/tmp/recent.json"));
      // Give the resolved .then() a tick to run; started must remain false.
      await Promise.resolve();
      expect(useOnboardingStore.getState().started).toBe(false);
    });
  });
});
