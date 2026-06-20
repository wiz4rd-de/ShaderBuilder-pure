import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";

import type { ProblemEntry } from "../store/documentStore";
import { useDocumentStore } from "../store/documentStore";
import { resetIdsForTest } from "../store/ids";
import { ProblemsPanel } from "./ProblemsPanel";

function store() {
  return useDocumentStore.getState();
}

function problem(passId: string, node: string, severity: "error" | "warning"): ProblemEntry {
  return {
    passId,
    passName: passId,
    diagnostic: { severity, code: "typeMismatch", message: `bad ${node}`, node, port: null },
  };
}

beforeEach(() => {
  resetIdsForTest();
  store().reset();
});

describe("ProblemsPanel", () => {
  it("shows the empty state with a Pipeline OK status when valid + no problems", () => {
    store().setCompileStatus({ diagnosticsByNode: {}, problems: [], valid: true, sourcesByPassId: {} });
    render(<ProblemsPanel />);
    expect(screen.getByText("Pipeline OK")).toBeInTheDocument();
    expect(screen.getByText("No problems.")).toBeInTheDocument();
  });

  it("flags an invalid pipeline and lists each problem with its pass + code", () => {
    store().setCompileStatus({
      diagnosticsByNode: { n1: [problem("g", "n1", "error").diagnostic] },
      problems: [problem("g", "n1", "error"), problem("g", "n2", "warning")],
      valid: false,
      sourcesByPassId: {},
    });
    render(<ProblemsPanel />);
    expect(screen.getByText("Pipeline not renderable")).toBeInTheDocument();
    expect(screen.getByText("1 error, 1 warning")).toBeInTheDocument();
    expect(screen.getByText("bad n1")).toBeInTheDocument();
    expect(screen.getByText("bad n2")).toBeInTheDocument();
  });

  it("clicking a problem drills into its pass and selects the offending node", () => {
    const passId = store().project.passes[0]!.id;
    store().setCompileStatus({
      diagnosticsByNode: { theNode: [problem(passId, "theNode", "error").diagnostic] },
      problems: [problem(passId, "theNode", "error")],
      valid: false,
      sourcesByPassId: {},
    });
    render(<ProblemsPanel />);
    fireEvent.click(screen.getByText("bad theNode"));
    expect(store().level).toBe("pass");
    expect(store().activePassId).toBe(passId);
    expect(store().selection.nodeIds).toEqual(["theNode"]);
  });

  it("renders an engine-synthesized render error alongside compile problems (#62)", () => {
    const passId = store().project.passes[0]!.id;
    store().pushEngineProblem({
      severity: "error",
      code: "slangCompile",
      message: "syntax error near 'foo'",
      passId,
      nodeId: null,
    });
    render(<ProblemsPanel />);
    expect(screen.getByText("syntax error near 'foo'")).toBeInTheDocument();
    expect(screen.getByText("engine")).toBeInTheDocument();
    // It still navigates to the owning pass (no node to select).
    fireEvent.click(screen.getByText("syntax error near 'foo'"));
    expect(store().activePassId).toBe(passId);
  });

  it("does not navigate for a pipeline-wide engine error with no pass (#62)", () => {
    store().pushEngineProblem({
      severity: "error",
      code: "deviceLost",
      message: "the GPU device was lost",
      passId: null,
      nodeId: null,
    });
    render(<ProblemsPanel />);
    const button = screen.getByText("the GPU device was lost").closest("button")!;
    expect(button).toBeDisabled();
  });

  it("clears engine problems when a fresh compile lands (#62)", () => {
    store().pushEngineProblem({
      severity: "error",
      code: "slangCompile",
      message: "boom",
      passId: null,
      nodeId: null,
    });
    expect(store().engineProblems).toHaveLength(1);
    store().setCompileStatus({ diagnosticsByNode: {}, problems: [], valid: true, sourcesByPassId: {} });
    expect(store().engineProblems).toHaveLength(0);
  });
});
