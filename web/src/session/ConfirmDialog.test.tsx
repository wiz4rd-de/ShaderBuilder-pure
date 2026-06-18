import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";

import { ConfirmDialog } from "./ConfirmDialog";
import { useConfirmStore } from "./confirmStore";

beforeEach(() => {
  useConfirmStore.setState({ prompt: null });
});

describe("ConfirmDialog", () => {
  it("renders nothing when there is no prompt", () => {
    const { container } = render(<ConfirmDialog />);
    expect(container.firstChild).toBeNull();
  });

  it("renders the live prompt with its labels and answers on click", async () => {
    render(<ConfirmDialog />);
    const p = useConfirmStore
      .getState()
      .ask("Save changes?", { confirm: "Save", discard: "Discard", cancel: "Cancel" });

    expect(await screen.findByText("Save changes?")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    await expect(p).resolves.toBe("discard");
  });

  it("omits the cancel button for a 2-button (recovery) prompt", async () => {
    render(<ConfirmDialog />);
    useConfirmStore.getState().ask("Recover?", { confirm: "Restore", discard: "Discard" });
    expect(await screen.findByText("Recover?")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Cancel" })).toBeNull();
  });

  it("Escape answers cancel when a cancel button exists", async () => {
    render(<ConfirmDialog />);
    const p = useConfirmStore
      .getState()
      .ask("Q", { confirm: "A", discard: "B", cancel: "C" });
    // Wait for the dialog to render so its Escape listener is attached.
    await screen.findByText("Q");
    fireEvent.keyDown(window, { key: "Escape" });
    await expect(p).resolves.toBe("cancel");
  });
});
