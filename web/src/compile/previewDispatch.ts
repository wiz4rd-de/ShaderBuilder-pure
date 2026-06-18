// Live-preview dispatch (#54) — push a project's freshly-compiled slang to the
// engine so the preview reflects the new modules.
//
// Two engine commands (added in Rust by #54): `load_shader_source` for a single
// renderable pass, `load_chain_sources` for a multi-pass chain. We only dispatch
// when EVERY pass produced a source (a globally-valid pipeline); a pipeline with a
// blocking error is NOT silently rendered as the last-good shader — the caller
// flags it invalid and the engine simply keeps the previous frame.
//
// The dispatch is injectable (`invoke`) so it unit-tests without a Tauri runtime.
import { invoke } from "@tauri-apps/api/core";

import type { PassSettings } from "../bindings/PassSettings";
import type { ProjectCompileResult } from "./compileLoop";

/** One in-memory chain pass sent to `load_chain_sources` (matches the Rust input). */
export interface ChainPassInput {
  source: string;
  settings: PassSettings;
}

/** The injected IPC caller (the hook passes the real Tauri `invoke`). */
export type Invoke = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;

/**
 * Push a compiled project to the engine preview. No-op (returns `false`) when the
 * pipeline is not globally valid — an unrenderable pipeline must NOT be rendered as
 * if it were clean; the engine holds its previous chain and the editor flags the
 * problem. Returns `true` when a chain/shader was dispatched.
 *
 * Single renderable pass → `load_shader_source(source)`. Multiple → the in-memory
 * `load_chain_sources(passes)` with each pass's generated source + settings.
 */
export async function dispatchPreview(
  result: ProjectCompileResult,
  invokeIpc: Invoke = invoke,
): Promise<boolean> {
  if (!result.valid || result.passes.length === 0) {
    return false;
  }
  // Every pass has a non-null source (guaranteed by `result.valid`).
  if (result.passes.length === 1) {
    const only = result.passes[0]!;
    await invokeIpc("load_shader_source", { source: only.source! });
    return true;
  }
  const passes: ChainPassInput[] = result.passes.map((p) => ({
    source: p.source!,
    settings: p.settings,
  }));
  await invokeIpc("load_chain_sources", { passes });
  return true;
}
