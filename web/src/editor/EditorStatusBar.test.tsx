import { act, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";

import { useDocumentStore } from "../store/documentStore";
import { resetIdsForTest } from "../store/ids";
import { EditorStatusBar } from "./EditorStatusBar";

function store() {
  return useDocumentStore.getState();
}

beforeEach(() => {
  resetIdsForTest();
  store().reset();
});

describe("EditorStatusBar — compile indicator (#54)", () => {
  it("shows a pending dash before the first compile", () => {
    render(<EditorStatusBar />);
    const compile = screen.getByTestId("status-compile");
    expect(compile).toHaveTextContent("—");
    expect(compile).toHaveAttribute("data-valid", "pending");
  });

  it("shows Valid when the pipeline is renderable", () => {
    act(() =>
      store().setCompileStatus({ diagnosticsByNode: {}, problems: [], valid: true }),
    );
    render(<EditorStatusBar />);
    const compile = screen.getByTestId("status-compile");
    expect(compile).toHaveTextContent("Valid");
    expect(compile).toHaveAttribute("data-valid", "true");
  });

  it("unmistakably flags an invalid pipeline with its problem count", () => {
    act(() =>
      store().setCompileStatus({
        diagnosticsByNode: { n: [] },
        problems: [
          {
            passId: "p",
            passName: "p",
            diagnostic: { severity: "error", code: "cycle", message: "c", node: "n", port: null },
          },
        ],
        valid: false,
      }),
    );
    render(<EditorStatusBar />);
    const compile = screen.getByTestId("status-compile");
    expect(compile).toHaveTextContent("Invalid (1)");
    expect(compile).toHaveAttribute("data-valid", "false");
  });

  it("shows a compiling hint while a compile is in flight", () => {
    act(() => store().setCompiling(true));
    render(<EditorStatusBar />);
    expect(screen.getByTestId("status-compile")).toHaveTextContent("Compiling");
  });
});
