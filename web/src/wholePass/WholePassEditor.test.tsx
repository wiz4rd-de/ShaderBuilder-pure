// Component tests for the whole-pass code editor (#52): rendering an opaque
// pass's source, the coalesced source-edit path (one undo entry), the scanned
// parameter/reference summary (mocked scan_pass_source), and convert-to-graph.
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

// Mock the scan command — return a fixed parameter + reference set so the summary
// is deterministic regardless of body text.
const invoke = vi.fn((cmd: string): Promise<unknown> => {
  if (cmd === "scan_pass_source") {
    return Promise.resolve({
      parameters: [
        { name: "BRIGHT", label: "Brightness", default: 1, min: 0, max: 2, step: 0.01 },
      ],
      references: [{ name: "Source", kind: "source" }],
    });
  }
  return Promise.resolve();
});
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => invoke(cmd, args),
}));

import { useDocumentStore } from "../store/documentStore";
import { resetIdsForTest } from "../store/ids";
import { WholePassEditor } from "./WholePassEditor";

function store() {
  return useDocumentStore.getState();
}

const SRC = "#version 450\nvoid main() { FragColor = texture(Source, vTexCoord); }\n";

beforeEach(() => {
  invoke.mockClear();
  resetIdsForTest();
  store().reset();
  const passId = store().project.passes[0]!.id;
  store().setPassToWholePassCode(passId, SRC);
  store().openPass(passId);
});

describe("WholePassEditor (#52)", () => {
  it("renders the opaque pass source in the code editor", () => {
    render(<WholePassEditor />);
    const code = screen.getByLabelText("Pass source") as HTMLTextAreaElement;
    expect(code.value).toBe(SRC);
    expect(screen.getByText("whole-pass code")).toBeTruthy();
  });

  it("scans the source and shows declared parameters + texture references", async () => {
    render(<WholePassEditor />);
    await waitFor(() => expect(screen.getByText("BRIGHT")).toBeTruthy());
    expect(screen.getByText("Source")).toBeTruthy();
    // The scan command was invoked with the current source.
    const scanCalls = invoke.mock.calls.filter(([c]) => c === "scan_pass_source");
    expect(scanCalls.length).toBeGreaterThan(0);
  });

  it("edits the source as one coalesced undo entry on blur", () => {
    render(<WholePassEditor />);
    const code = screen.getByLabelText("Pass source") as HTMLTextAreaElement;
    const undoDepth = store().past.length;
    fireEvent.change(code, { target: { value: SRC + "// a" } });
    fireEvent.change(code, { target: { value: SRC + "// ab" } });
    fireEvent.blur(code);
    const pass = store().project.passes[0]!;
    if (pass.source.kind === "wholePassCode") {
      expect(pass.source.source).toBe(SRC + "// ab");
    }
    expect(store().past.length).toBe(undoDepth + 1);
  });

  it("convert-to-node-graph switches the pass back to a graph", () => {
    render(<WholePassEditor />);
    fireEvent.click(screen.getByText("Convert to node graph"));
    expect(store().project.passes[0]!.source.kind).toBe("graph");
  });
});
