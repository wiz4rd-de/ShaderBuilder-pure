// Live-preview dispatch (#54) — push a project's freshly-compiled slang to the
// engine so the preview reflects the new modules.
//
// We dispatch the whole chain through the SETTINGS-AWARE `load_chain_sources`
// command (added in Rust by #54), which carries each pass's generated source AND
// its `PassSettings` (scale/filter/wrap/format/mipmap/frame_count_mod). The Rust
// `set_chain` / `load_chain_sources` handles a 1-element chain identically, so a
// single renderable pass is sent the SAME way (not via the settings-blind
// `load_shader_source`, which dropped the pass's scale/format/wrap — #4-review).
// We only dispatch when EVERY pass produced a source (a globally-valid pipeline);
// a pipeline with a blocking error is NOT silently rendered as the last-good
// shader — the caller flags it invalid and the engine simply keeps the previous
// frame.
//
// The dispatch is injectable (`invoke`) so it unit-tests without a Tauri runtime.
import { invoke } from "@tauri-apps/api/core";

import type { PassSettings } from "../bindings/PassSettings";
import type { ProjectCompileResult } from "./compileLoop";

/** One in-memory chain pass sent to `load_chain_sources` (matches the Rust input). */
export interface ChainPassInput {
  source: string;
  settings: PassSettings;
  /**
   * The owning pipeline pass id (#62) so a slang-compile failure on THIS pass
   * maps to a pass-tagged engine error the editor can surface against the right
   * pass (the whole-pass-code path has no node-IR to catch it earlier).
   */
  passId: string;
}

/** The injected IPC caller (the hook passes the real Tauri `invoke`). */
export type Invoke = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;

/**
 * Push a compiled project to the engine preview. No-op (returns `false`) when the
 * pipeline is not globally valid — an unrenderable pipeline must NOT be rendered as
 * if it were clean; the engine holds its previous chain and the editor flags the
 * problem. Returns `true` when a chain was dispatched.
 *
 * Every renderable pass (including a single one) goes through the settings-aware
 * `load_chain_sources(passes)`, carrying each pass's generated source + its
 * `PassSettings` so scale/filter/wrap/format are honored (#4-review).
 */
export async function dispatchPreview(
  result: ProjectCompileResult,
  invokeIpc: Invoke = invoke,
): Promise<boolean> {
  if (!result.valid || result.passes.length === 0) {
    return false;
  }
  // Every pass has a non-null source (guaranteed by `result.valid`). Route the
  // whole chain — even a single pass — through the settings-aware command so the
  // pass's PassSettings (scale/filter/wrap/format/mipmap/frame_count_mod) are
  // applied; `load_chain_sources` handles a 1-element chain identically.
  const passes: ChainPassInput[] = result.passes.map((p) => ({
    source: p.source!,
    settings: p.settings,
    passId: p.passId,
  }));
  await invokeIpc("load_chain_sources", { passes });
  return true;
}
