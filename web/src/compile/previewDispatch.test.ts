import { describe, expect, it, vi } from "vitest";

import type { PassSettings } from "../bindings/PassSettings";
import type { ProjectCompileResult } from "./compileLoop";
import { dispatchPreview } from "./previewDispatch";

function settings(): PassSettings {
  return {
    scaleX: { scaleType: null, scale: null },
    scaleY: { scaleType: null, scale: null },
    filterLinear: null,
    wrapMode: null,
    mipmapInput: null,
    floatFramebuffer: null,
    srgbFramebuffer: null,
    alias: null,
    frameCountMod: null,
  };
}

function result(
  passes: Array<{ passId: string; source: string | null }>,
  valid: boolean,
): ProjectCompileResult {
  return {
    passes: passes.map((p) => ({ ...p, settings: settings(), diagnostics: [] })),
    diagnosticsByNode: {},
    problems: [],
    valid,
  };
}

describe("dispatchPreview", () => {
  it("sends a single renderable pass via the settings-aware load_chain_sources", async () => {
    // A single pass MUST still carry its PassSettings (scale/filter/wrap/format) —
    // it goes through load_chain_sources like any chain, NOT the settings-blind
    // load_shader_source which dropped them (#4-review).
    const invoke = vi.fn(async () => undefined);
    const dispatched = await dispatchPreview(result([{ passId: "a", source: "// s" }], true), invoke);
    expect(dispatched).toBe(true);
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith("load_chain_sources", {
      passes: [{ source: "// s", settings: settings() }],
    });
  });

  it("sends a multi-pass chain via load_chain_sources in pipeline order", async () => {
    const invoke = vi.fn(async () => undefined);
    await dispatchPreview(
      result(
        [
          { passId: "a", source: "// a" },
          { passId: "b", source: "// b" },
        ],
        true,
      ),
      invoke,
    );
    expect(invoke).toHaveBeenCalledWith("load_chain_sources", {
      passes: [
        { source: "// a", settings: settings() },
        { source: "// b", settings: settings() },
      ],
    });
  });

  it("does NOT dispatch when the pipeline is invalid (preview is not rendered)", async () => {
    const invoke = vi.fn(async () => undefined);
    const dispatched = await dispatchPreview(
      result([{ passId: "a", source: null }], false),
      invoke,
    );
    expect(dispatched).toBe(false);
    expect(invoke).not.toHaveBeenCalled();
  });

  it("does NOT dispatch an empty pipeline", async () => {
    const invoke = vi.fn(async () => undefined);
    expect(await dispatchPreview(result([], true), invoke)).toBe(false);
    expect(invoke).not.toHaveBeenCalled();
  });
});
