import { fireEvent, render } from "@testing-library/react";
import { createRef } from "react";
import { beforeEach, describe, expect, it } from "vitest";

import { SplitDivider } from "./SplitDivider";
import { useCompareStore } from "./compareStore";

beforeEach(() => {
  useCompareStore.setState({
    mode: "split",
    orientation: "vertical",
    splitPos: 0.5,
    reference: null,
  });
});

/**
 * Render the divider inside a pane whose geometry is stubbed (jsdom returns a
 * zero rect otherwise), so the pointer-to-normalized math is exercised for real.
 */
function renderDivider(orientation: "vertical" | "horizontal") {
  const paneRef = createRef<HTMLDivElement>();
  const utils = render(
    <div
      ref={paneRef}
      style={{ position: "relative" }}
      data-testid="pane"
    >
      <SplitDivider paneRef={paneRef} orientation={orientation} pos={0.5} />
    </div>,
  );
  // Stub the pane's box: 200px wide, 100px tall, at the viewport origin.
  paneRef.current!.getBoundingClientRect = () =>
    ({ left: 0, top: 0, width: 200, height: 100, right: 200, bottom: 100, x: 0, y: 0 }) as DOMRect;
  return { ...utils, paneRef };
}

describe("SplitDivider", () => {
  it("pointer-down sets the divider to the press point (vertical)", () => {
    const { getByTestId } = renderDivider("vertical");
    // 50px into a 200px-wide pane -> 0.25.
    fireEvent.pointerDown(getByTestId("split-divider"), { clientX: 50, clientY: 10 });
    expect(useCompareStore.getState().splitPos).toBeCloseTo(0.25, 6);
  });

  it("dragging updates the position smoothly while the pointer is down", () => {
    const { getByTestId } = renderDivider("vertical");
    fireEvent.pointerDown(getByTestId("split-divider"), { clientX: 100, clientY: 10 });
    expect(useCompareStore.getState().splitPos).toBeCloseTo(0.5, 6);
    // A window-level move while dragging tracks the pointer.
    fireEvent.pointerMove(window, { clientX: 150, clientY: 10 });
    expect(useCompareStore.getState().splitPos).toBeCloseTo(0.75, 6);
  });

  it("a move AFTER pointer-up does not move the divider", () => {
    const { getByTestId } = renderDivider("vertical");
    fireEvent.pointerDown(getByTestId("split-divider"), { clientX: 100, clientY: 10 });
    fireEvent.pointerUp(window);
    fireEvent.pointerMove(window, { clientX: 20, clientY: 10 });
    // Still at the press point — the drag ended.
    expect(useCompareStore.getState().splitPos).toBeCloseTo(0.5, 6);
  });

  it("clamps a drag past the pane edge to [0,1]", () => {
    const { getByTestId } = renderDivider("vertical");
    fireEvent.pointerDown(getByTestId("split-divider"), { clientX: 500, clientY: 10 });
    expect(useCompareStore.getState().splitPos).toBe(1);
  });

  it("uses the Y axis when horizontal", () => {
    const { getByTestId } = renderDivider("horizontal");
    // 25px into a 100px-tall pane -> 0.25.
    fireEvent.pointerDown(getByTestId("split-divider"), { clientX: 10, clientY: 25 });
    expect(useCompareStore.getState().splitPos).toBeCloseTo(0.25, 6);
  });
});
