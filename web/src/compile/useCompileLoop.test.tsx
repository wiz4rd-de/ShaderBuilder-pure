import { act, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(() => Promise.resolve()) }));

import type { CompileGraphResult } from "../bindings/CompileGraphResult";
import { useDocumentStore } from "../store/documentStore";
import { resetIdsForTest } from "../store/ids";
import type { InvokeCompile } from "./compileLoop";
import type { Invoke } from "./previewDispatch";
import { useCompileLoop } from "./useCompileLoop";

function store() {
  return useDocumentStore.getState();
}

/** A host that runs the loop with injected fakes + an immediate (0ms) debounce. */
function Host({
  compile,
  invokeIpc,
}: {
  compile: InvokeCompile;
  invokeIpc: Invoke;
}): React.JSX.Element {
  useCompileLoop({ debounceMs: 1, invokeCompile: compile, invokeIpc });
  return <div />;
}

const cleanResult = (source = "// gen"): CompileGraphResult => ({
  source,
  diagnostics: { items: [] },
});

beforeEach(() => {
  resetIdsForTest();
  store().reset();
  // Drill into the single graph pass so addNode targets it.
  store().openPass(store().project.passes[0]!.id);
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
  vi.clearAllMocks();
});

describe("useCompileLoop", () => {
  it("debounces an edit burst into ONE compile (no per-keystroke storm)", async () => {
    const compile = vi.fn<InvokeCompile>(async () => cleanResult());
    const invokeIpc = vi.fn<Invoke>(async () => undefined);
    render(<Host compile={compile} invokeIpc={invokeIpc} />);

    // Mount schedules an initial compile; flush it.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    compile.mockClear();

    // Three rapid edits within the debounce window → one compile.
    act(() => {
      store().addNode("source", { x: 0, y: 0 });
      store().addNode("output", { x: 0, y: 0 });
      store().addNode("texcoord", { x: 0, y: 0 });
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(compile).toHaveBeenCalledTimes(1);
  });

  it("writes node-keyed diagnostics + marks the pipeline invalid on a type error", async () => {
    const compile = vi.fn<InvokeCompile>(async () => ({
      source: null,
      diagnostics: {
        items: [
          { severity: "error", code: "typeMismatch", message: "bad", node: "n1", port: null },
        ],
      },
    }));
    const invokeIpc = vi.fn<Invoke>(async () => undefined);

    // The mocked compile_graph reports a diagnostic on IrNode id "n1"; the loop
    // keys it verbatim (compile_graph owns the offending node id).
    act(() => {
      store().addNode("output", { x: 0, y: 0 });
    });

    render(<Host compile={compile} invokeIpc={invokeIpc} />);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2);
    });

    expect(store().pipelineValid).toBe(false);
    // The diagnostic node id "n1" is keyed verbatim (compile_graph owns node ids).
    expect(store().diagnosticsByNode["n1"]).toHaveLength(1);
    expect(store().problems).toHaveLength(1);
    // An invalid pipeline is NOT dispatched to the engine preview.
    expect(invokeIpc).not.toHaveBeenCalled();
  });

  it("dispatches the generated source to the engine on a clean compile", async () => {
    const compile = vi.fn<InvokeCompile>(async () => cleanResult("// generated"));
    const invokeIpc = vi.fn<Invoke>(async () => undefined);
    render(<Host compile={compile} invokeIpc={invokeIpc} />);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2);
    });
    expect(store().pipelineValid).toBe(true);
    expect(invokeIpc).toHaveBeenCalledWith("load_shader_source", { source: "// generated" });
    expect(store().diagnosticsByNode).toEqual({});
    expect(store().problems).toEqual([]);
  });

  it("clears stale diagnostics on the next clean compile", async () => {
    let call = 0;
    const compile = vi.fn<InvokeCompile>(async () => {
      call += 1;
      if (call === 1) {
        return {
          source: null,
          diagnostics: {
            items: [
              { severity: "error", code: "cycle", message: "c", node: "x", port: null },
            ],
          },
        };
      }
      return cleanResult();
    });
    const invokeIpc = vi.fn<Invoke>(async () => undefined);
    render(<Host compile={compile} invokeIpc={invokeIpc} />);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(2);
    });
    expect(store().diagnosticsByNode["x"]).toBeDefined();

    // A new edit triggers a second (clean) compile that clears the stale diag.
    act(() => {
      store().addNode("source", { x: 0, y: 0 });
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2);
    });
    expect(store().diagnosticsByNode).toEqual({});
    expect(store().pipelineValid).toBe(true);
  });
});
