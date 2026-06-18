// Export a graph-authored project as a RetroArch `.slangp` bundle (#54).
//
// The Rust exporter (`export_preset`) only writes WHOLE-PASS slang verbatim — it
// throws `GraphPassUnsupported` for a graph pass. Per Decision Log #5 (generated
// source is OUTPUT-ONLY), the frontend bridges that gap: it compiles each graph
// pass (the SAME `compile_graph` the live preview ran), substitutes the generated
// slang as a `PassSource::WholePassCode`, then sends the all-whole-pass Project to
// `export_preset`. This is what makes #55's "displayed source == exported per-pass
// .slang" hold — the exported bytes are exactly the compiled+previewed bytes.
//
// An invalid pipeline (a graph pass that did not compile cleanly) is NOT exported:
// the result is a typed `notRenderable` outcome the caller surfaces, mirroring how
// the live preview refuses to render an invalid pipeline.
import { invoke } from "@tauri-apps/api/core";

import type { ExportError } from "../bindings/ExportError";
import type { ExportResult } from "../bindings/ExportResult";
import type { Project } from "../bindings/Project";
import {
  compileProject,
  type InvokeCompile,
  type ProjectCompileResult,
} from "./compileLoop";
import { sourcesFromCompile, substituteGraphPasses } from "./exportSubstitution";

/** The outcome of an export attempt (a typed union the caller branches on). */
export type ExportProjectOutcome =
  | { kind: "ok"; result: ExportResult }
  | { kind: "error"; error: ExportError }
  | { kind: "notRenderable"; uncompiledPassIds: string[] };

/** The injected IPC callers (the default uses the real Tauri `invoke`). */
export interface ExportDeps {
  invokeCompile?: InvokeCompile;
  exportPreset?: (project: Project, destDir: string) => Promise<ExportResult>;
}

const defaultCompile: InvokeCompile = (args) => invoke("compile_graph", args);
const defaultExport = (project: Project, destDir: string): Promise<ExportResult> =>
  invoke("export_preset", { project, destDir });

/**
 * Compile + substitute + export a project to `destDir`. Compiles every graph pass,
 * substitutes its generated slang as whole-pass code, and sends the result to
 * `export_preset`. Refuses (returns `notRenderable`) when any graph pass failed to
 * compile; surfaces a thrown `ExportError` as a typed `error` outcome.
 */
export async function exportProject(
  project: Project,
  destDir: string,
  deps: ExportDeps = {},
): Promise<ExportProjectOutcome> {
  const compile = deps.invokeCompile ?? defaultCompile;
  const exportPreset = deps.exportPreset ?? defaultExport;

  const compiled: ProjectCompileResult = await compileProject(project, compile);
  const substitution = substituteGraphPasses(
    project,
    sourcesFromCompile(compiled.passes),
  );
  if (!substitution.ok) {
    return { kind: "notRenderable", uncompiledPassIds: substitution.uncompiledPassIds };
  }

  try {
    const result = await exportPreset(substitution.project, destDir);
    return { kind: "ok", result };
  } catch (err) {
    // The command rejects with the typed ExportError shape over IPC.
    return { kind: "error", error: err as ExportError };
  }
}
