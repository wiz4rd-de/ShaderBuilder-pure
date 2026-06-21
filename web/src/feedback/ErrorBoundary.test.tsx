import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ErrorBoundary } from "./ErrorBoundary";

/**
 * Whether the child should throw on its next render. This lives at MODULE scope —
 * OUTSIDE the boundary subtree — so flipping it to false before clicking "Try
 * again" lets the boundary's `reset()` re-render the subtree to a healthy state.
 * The boundary re-reads it on every render attempt, so recovery is genuinely
 * proven: if `reset()` were a no-op the fallback would persist and the test fails.
 */
let shouldThrow = true;

/** A child that throws while `shouldThrow`, then renders fine once it is flipped. */
function Bomb(): React.JSX.Element {
  if (shouldThrow) {
    throw new Error("kaboom");
  }
  return <div>safe content</div>;
}

let spy: ReturnType<typeof vi.spyOn>;
beforeEach(() => {
  shouldThrow = true;
  // React logs the caught error to console.error; silence it for a clean run.
  spy = vi.spyOn(console, "error").mockImplementation(() => {});
});
afterEach(() => {
  spy.mockRestore();
});

describe("ErrorBoundary", () => {
  it("renders children when they do not throw", () => {
    render(
      <ErrorBoundary>
        <div>all good</div>
      </ErrorBoundary>,
    );
    expect(screen.getByText("all good")).toBeInTheDocument();
  });

  it("shows a recoverable fallback (not a blank window) when a child throws", () => {
    render(
      <ErrorBoundary label="Editor">
        <Bomb />
      </ErrorBoundary>,
    );
    expect(screen.getByTestId("error-boundary")).toBeInTheDocument();
    expect(screen.getByText("Editor hit an error")).toBeInTheDocument();
    expect(screen.getByText("kaboom")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Try again" })).toBeInTheDocument();
  });

  it("surfaces the error stack + component stack in a Technical details disclosure", () => {
    render(
      <ErrorBoundary label="Editor">
        <Bomb />
      </ErrorBoundary>,
    );
    expect(screen.getByText("Technical details")).toBeInTheDocument();
    const stack = screen.getByTestId("error-boundary-stack");
    // The error's own stack (Error: kaboom) is shown...
    expect(stack.textContent).toContain("kaboom");
    // ...and the React component stack (captured in componentDidCatch) names the
    // throwing component, which is what pinpoints a render-time loop/exception.
    expect(stack.textContent).toContain("Component stack:");
    expect(stack.textContent).toContain("Bomb");
  });

  it("recovers when 'Try again' is clicked after the cause is fixed", () => {
    render(
      <ErrorBoundary label="Editor">
        <Bomb />
      </ErrorBoundary>,
    );
    // The child threw -> the fallback is shown.
    expect(screen.getByTestId("error-boundary")).toBeInTheDocument();
    expect(screen.queryByText("safe content")).not.toBeInTheDocument();

    // Fix the cause from OUTSIDE the boundary subtree, THEN retry. The boundary's
    // `reset()` clears its error state and re-renders the (now healthy) child.
    shouldThrow = false;
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));

    // Recovery proven: the fallback is gone and the healthy child renders. This
    // FAILS if `reset()` is a no-op (the fallback would persist).
    expect(screen.queryByTestId("error-boundary")).not.toBeInTheDocument();
    expect(screen.getByText("safe content")).toBeInTheDocument();
  });
});
