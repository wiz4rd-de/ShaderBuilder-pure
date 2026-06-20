import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { substituteGraphPasses } from "../compile/exportSubstitution";
import { useDocumentStore } from "../store/documentStore";
import { resetIdsForTest } from "../store/ids";
import { GeneratedCodePanel } from "./GeneratedCodePanel";

function store() {
  return useDocumentStore.getState();
}

/** The first (default) pass id of the fresh project. */
function activePass(): string {
  return store().activePassId;
}

beforeEach(() => {
  resetIdsForTest();
  store().reset();
});

describe("GeneratedCodePanel", () => {
  it("labels itself read-only / output-only", () => {
    render(<GeneratedCodePanel />);
    expect(screen.getByText(/read-only/i)).toBeInTheDocument();
    expect(screen.getByText(/not parsed back into nodes/i)).toBeInTheDocument();
  });

  it("shows an empty marker before any compile", () => {
    render(<GeneratedCodePanel />);
    expect(screen.getByText(/has not compiled/i)).toBeInTheDocument();
  });

  it("renders the active pass's generated slang after a successful compile", () => {
    const id = activePass();
    store().setCompileStatus({
      diagnosticsByNode: {},
      problems: [],
      valid: true,
      sourcesByPassId: { [id]: "#version 450\nvec4 main(){ return texture(s, v); }" },
    });
    render(<GeneratedCodePanel />);
    const pre = screen.getByLabelText("Generated slang");
    expect(pre.textContent).toContain("#version 450");
    expect(pre.textContent).toContain("texture");
    // It is a <pre>, not an input/textarea — read-only by construction.
    expect(pre.tagName).toBe("PRE");
    expect(screen.queryByRole("textbox")).toBeNull();
  });

  it("switches the shown source when the active pass changes", () => {
    const first = activePass();
    const second = store().addPass("Second");
    store().setCompileStatus({
      diagnosticsByNode: {},
      problems: [],
      valid: true,
      sourcesByPassId: { [first]: "// FIRST PASS", [second]: "// SECOND PASS" },
    });
    // addPass made `second` active.
    const { rerender } = render(<GeneratedCodePanel />);
    expect(screen.getByLabelText("Generated slang").textContent).toContain("SECOND PASS");

    useDocumentStore.setState({ activePassId: first });
    rerender(<GeneratedCodePanel />);
    expect(screen.getByLabelText("Generated slang").textContent).toContain("FIRST PASS");
  });

  it("shows the last-good source with a stale banner when the pass no longer compiles", () => {
    const id = activePass();
    // First a good compile, then a failing one (source null).
    store().setCompileStatus({
      diagnosticsByNode: {},
      problems: [],
      valid: true,
      sourcesByPassId: { [id]: "// GOOD SOURCE" },
    });
    store().setCompileStatus({
      diagnosticsByNode: {},
      problems: [],
      valid: false,
      sourcesByPassId: { [id]: null },
    });
    render(<GeneratedCodePanel />);
    expect(screen.getByRole("status")).toHaveTextContent(/does not currently compile/i);
    expect(screen.getByLabelText("Generated slang").textContent).toContain("GOOD SOURCE");
  });

  it("copies the shown source to the clipboard", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    const id = activePass();
    store().setCompileStatus({
      diagnosticsByNode: {},
      problems: [],
      valid: true,
      sourcesByPassId: { [id]: "// COPY ME" },
    });
    render(<GeneratedCodePanel />);
    fireEvent.click(screen.getByText("Copy"));
    expect(writeText).toHaveBeenCalledWith("// COPY ME");
    await waitFor(() => expect(screen.getByText("Copied")).toBeInTheDocument());
  });

  // The "matches export" proof (#55 acceptance): the source the viewer shows for a
  // pass is the SAME string the Phase-3 export bundle embeds for that pass — both
  // come from `compile_graph`'s per-pass source. We feed the store's per-pass
  // source through the export substitution and assert the embedded source equals
  // what the viewer displays.
  it("displays exactly what export substitution would embed for the pass", () => {
    const id = activePass();
    const generated = "#version 450\n#pragma name doubled\nvec4 main(){ return c*2.0; }";
    store().setCompileStatus({
      diagnosticsByNode: {},
      problems: [],
      valid: true,
      sourcesByPassId: { [id]: generated },
    });

    // What the viewer shows:
    render(<GeneratedCodePanel />);
    const displayed = screen.getByLabelText("Generated slang").textContent;

    // What export would write for the same pass, from the same per-pass source map:
    const sub = substituteGraphPasses(store().project, { [id]: generated });
    expect(sub.ok).toBe(true);
    if (!sub.ok) return;
    const exported = sub.project.passes.find((p) => p.id === id)!.source;
    expect(exported.kind).toBe("wholePassCode");
    if (exported.kind !== "wholePassCode") return;

    // The bytes are identical by construction — the viewer round-trips the source.
    expect(displayed).toBe(exported.source);
  });

  it("explains that a whole-pass code pass has no generated source", () => {
    const id = activePass();
    store().setPassToWholePassCode(id, "// verbatim user code");
    render(<GeneratedCodePanel />);
    expect(screen.getByText(/whole-pass code pass/i)).toBeInTheDocument();
  });
});
