// Typed IPC wrapper for the `scan_pass_source` Tauri command (#52). Scans a
// whole-pass `.slang` source STRING for its `#pragma parameter` declarations +
// referenced RetroArch textures, reusing the Rust preset-io scanners (so the
// recognised-semantics taxonomy stays in ONE place, Rust). The result drives the
// parameter sliders (#53) + pipeline-view wiring (#46) for an opaque pass.
import { invoke } from "@tauri-apps/api/core";

import type { Parameter } from "../bindings/Parameter";
import type { TextureRef } from "../bindings/TextureRef";

/** The shape `scan_pass_source` returns (mirrors crates/app/src/scan.rs ScanResult).
 *  Hand-declared because ScanResult is intentionally NOT a #[ts(export)] type. */
export interface ScanPassResult {
  /** The `#pragma parameter` declarations, in declaration order. */
  parameters: Parameter[];
  /** The referenced RetroArch textures/aliases, deduplicated + sorted by name. */
  references: TextureRef[];
}

/**
 * Scan a whole-pass `.slang` `source` for its declared parameters + texture
 * references. `aliases`/`luts` are the preset-known names so a referenced alias
 * is classified as such (default: none). Returns `{parameters:[], references:[]}`
 * when the Tauri runtime is unavailable (e.g. unit tests) instead of throwing, so
 * a component can render before the command resolves.
 */
export async function scanPassSource(
  source: string,
  aliases: string[] = [],
  luts: string[] = [],
): Promise<ScanPassResult> {
  try {
    return await invoke<ScanPassResult>("scan_pass_source", { source, aliases, luts });
  } catch {
    return { parameters: [], references: [] };
  }
}
