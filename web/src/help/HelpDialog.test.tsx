import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { HelpDialog } from "./HelpDialog";

describe("HelpDialog", () => {
  it("lists the keyboard-shortcut reference and the user-guide pointer", () => {
    render(<HelpDialog onClose={() => undefined} />);
    // A few representative shortcuts across groups.
    expect(screen.getByText("Undo")).toBeInTheDocument();
    expect(screen.getByText("Save As")).toBeInTheDocument();
    expect(screen.getByText("Box-select")).toBeInTheDocument();
    // The user-guide pointer.
    expect(screen.getByText(/docs\/user-guide\.md/)).toBeInTheDocument();
  });

  it("closes on the × button and on the backdrop", () => {
    const onClose = vi.fn();
    const { rerender } = render(<HelpDialog onClose={onClose} />);
    fireEvent.click(screen.getByRole("button", { name: "Close help" }));
    expect(onClose).toHaveBeenCalledOnce();

    onClose.mockClear();
    rerender(<HelpDialog onClose={onClose} />);
    // Clicking the backdrop (the dialog role element) closes; clicking inside does not.
    fireEvent.click(screen.getByRole("dialog", { name: "Help" }));
    expect(onClose).toHaveBeenCalledOnce();
  });
});
