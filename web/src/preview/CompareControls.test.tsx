import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { CompareControls } from "./CompareControls";
import { useCompareStore } from "./compareStore";

/** A 2x2 solid ImageData to stand in for a captured reference. */
function fakeFrame(): ImageData {
  return new ImageData(new Uint8ClampedArray(2 * 2 * 4), 2, 2);
}

/** Reset compare state to its initial values before each test. */
beforeEach(() => {
  useCompareStore.setState({
    mode: "live",
    orientation: "vertical",
    splitPos: 0.5,
    reference: null,
  });
});

describe("CompareControls", () => {
  it("disables the compare controls when no reference is captured (inert, not erroring)", () => {
    render(<CompareControls hasLiveFrame onSetReference={vi.fn()} />);
    // Reference / Split modes + Clear are disabled until a reference exists.
    expect(screen.getByRole("radio", { name: "Reference" })).toBeDisabled();
    expect(screen.getByRole("radio", { name: "Split" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Clear" })).toBeDisabled();
    // Live is always available.
    expect(screen.getByRole("radio", { name: "Live" })).not.toBeDisabled();
    expect(screen.getByText("No reference")).toBeInTheDocument();
  });

  it("'Set reference' is gated on a live frame existing", () => {
    const onSetReference = vi.fn();
    const { rerender } = render(
      <CompareControls hasLiveFrame={false} onSetReference={onSetReference} />,
    );
    expect(screen.getByRole("button", { name: "Set reference" })).toBeDisabled();

    rerender(<CompareControls hasLiveFrame onSetReference={onSetReference} />);
    fireEvent.click(screen.getByRole("button", { name: "Set reference" }));
    expect(onSetReference).toHaveBeenCalledOnce();
  });

  it("A/B flip swaps the active mode instantly and labels which is shown", () => {
    useCompareStore.getState().setReference(fakeFrame());
    render(<CompareControls hasLiveFrame onSetReference={vi.fn()} />);

    expect(screen.getByText("Showing: Live")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("radio", { name: "Reference" }));
    expect(useCompareStore.getState().mode).toBe("reference");
    expect(screen.getByText("Showing: Reference")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("radio", { name: "Live" }));
    expect(useCompareStore.getState().mode).toBe("live");
    expect(screen.getByText("Showing: Live")).toBeInTheDocument();
  });

  it("entering split mode exposes the orientation toggle and a split label", () => {
    useCompareStore.getState().setReference(fakeFrame());
    render(<CompareControls hasLiveFrame onSetReference={vi.fn()} />);

    fireEvent.click(screen.getByRole("radio", { name: "Split" }));
    expect(useCompareStore.getState().mode).toBe("split");
    expect(screen.getByText("Split: Reference │ Live")).toBeInTheDocument();

    const orient = screen.getByRole("button", { name: "Vertical" });
    fireEvent.click(orient);
    expect(useCompareStore.getState().orientation).toBe("horizontal");
  });

  it("'Clear' discards the reference and falls back to live", () => {
    useCompareStore.getState().setReference(fakeFrame());
    useCompareStore.getState().setMode("reference");
    render(<CompareControls hasLiveFrame onSetReference={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "Clear" }));
    expect(useCompareStore.getState().reference).toBeNull();
    expect(useCompareStore.getState().mode).toBe("live");
    expect(screen.getByText("No reference")).toBeInTheDocument();
  });
});
