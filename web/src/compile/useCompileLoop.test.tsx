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
    // The single graph pass is pushed through the settings-aware chain command
    // (carrying its PassSettings), not the settings-blind load_shader_source.
    const call = invokeIpc.mock.calls.find((c) => c[0] === "load_chain_sources");
    expect(call).toBeDefined();
    const passes = (call![1] as { passes: Array<{ source: string }> }).passes;
    expect(passes).toHaveLength(1);
    expect(passes[0]!.source).toBe("// generated");
    expect(store().diagnosticsByNode).toEqual({});
    expect(store().problems).toEqual([]);
  });

  it("drops a stale out-of-order compile result (slow A, then fast B)", async () => {
    // Edit A is dispatched first but resolves LAST; edit B is dispatched second and
    // resolves FIRST. The loop tags each dispatch with an edit seq and DROPS a
    // resolved result whose seq is no longer the latest. So when A (the older,
    // slower compile) finally resolves, its stale diagnostics/source must NOT
    // overwrite B's — the store must end with B's result, and the last preview
    // dispatch must carry B's source.
    const deferred = <T,>() => {
      let resolve!: (v: T) => void;
      const promise = new Promise<T>((r) => {
        resolve = r;
      });
      return { promise, resolve };
    };
    const dA = deferred<CompileGraphResult>();
    const dB = deferred<CompileGraphResult>();

    let call = 0;
    const compile = vi.fn<InvokeCompile>(() => {
      call += 1;
      return call === 1 ? dA.promise : dB.promise;
    });
    const invokeIpc = vi.fn<Invoke>(async () => undefined);
    render(<Host compile={compile} invokeIpc={invokeIpc} />);

    // Dispatch A (the mount compile) by flushing the debounce.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2);
    });
    // A new edit dispatches B (a second, newer compile) while A is still in flight.
    act(() => {
      store().addNode("source", { x: 0, y: 0 });
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2);
    });
    expect(compile).toHaveBeenCalledTimes(2);

    // B (the newer edit) resolves FIRST with a clean source.
    await act(async () => {
      dB.resolve(cleanResult("// from B"));
      await Promise.resolve();
    });
    // THEN A (the older, slower edit) resolves with a STALE error result.
    await act(async () => {
      dA.resolve({
        source: null,
        diagnostics: {
          items: [
            { severity: "error", code: "cycle", message: "stale A", node: "a1", port: null },
          ],
        },
      });
      await Promise.resolve();
    });

    // The store reflects B (clean), NOT A's stale error — A was dropped.
    expect(store().pipelineValid).toBe(true);
    expect(store().diagnosticsByNode).toEqual({});
    expect(store().problems).toEqual([]);
    // The last preview dispatch carried B's source (A never dispatched a preview).
    const chainCalls = invokeIpc.mock.calls.filter((c) => c[0] === "load_chain_sources");
    expect(chainCalls.length).toBeGreaterThan(0);
    const last = chainCalls[chainCalls.length - 1]!;
    const passes = (last[1] as { passes: Array<{ source: string }> }).passes;
    expect(passes[0]!.source).toBe("// from B");
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
