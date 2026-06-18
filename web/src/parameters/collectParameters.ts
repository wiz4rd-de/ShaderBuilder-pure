// Parameter collection (#53) — gather EVERY `#pragma parameter` knob declared
// across the active pipeline into one global, de-duplicated slider list.
//
// RetroArch parameters are GLOBAL by id: the same `#pragma parameter` name may be
// declared in several passes, but it is ONE runtime knob driving ONE UBO slot. So
// we collect by `name`, first declaration wins, in a stable order. Sources, in
// precedence order (first wins on a name clash):
//
//   1. Project.parameters       — authored/imported project-level declarations
//                                  (these carry the canonical export defaults, so
//                                  they take precedence on label/range/default).
//   2. each graph Pass          — its Param nodes (via graphToIr) AND its
//                                  Pass.parameters list.
//   3. each whole-pass Pass     — the `#pragma parameter`s recovered by the Rust
//                                  `scan_pass_source` command (passed in as a map,
//                                  since scanning is async/IPC and not pure).
//
// Pure + synchronous: the async whole-pass scan results are supplied by the caller
// as `wholePassParams` keyed by pass id, so this stays unit-testable with no Tauri.
import type { Parameter } from "../bindings/Parameter";
import type { Project } from "../bindings/Project";
import { graphToIr } from "../nodes/graphToIr";

/**
 * Collect the global, de-duplicated parameter list for a project. `wholePassParams`
 * maps a whole-pass code pass id → the parameters its source declares (from
 * `scan_pass_source`); a missing entry just contributes nothing for that pass.
 *
 * De-duplication is by `name`, FIRST declaration wins, preserving discovery order
 * (project-level first, then per-pass in pipeline order). The result is what the
 * slider panel renders and what export reconciles against.
 */
export function collectParameters(
  project: Project,
  wholePassParams: Record<string, Parameter[]> = {},
): Parameter[] {
  const out: Parameter[] = [];
  const seen = new Set<string>();

  const add = (param: Parameter | null | undefined): void => {
    if (!param || param.name.length === 0 || seen.has(param.name)) {
      return;
    }
    seen.add(param.name);
    out.push(param);
  };

  // 1. Project-level declarations (canonical defaults for export).
  for (const p of project.parameters) {
    add(p);
  }

  // 2 + 3. Each pass, in pipeline order.
  for (const pass of project.passes) {
    if (pass.source.kind === "graph") {
      // Param nodes declared inside the graph.
      for (const p of graphToIr(pass.source.graph).parameters) {
        add(p);
      }
    } else {
      // Whole-pass code: the scanned `#pragma parameter`s for this pass id.
      for (const p of wholePassParams[pass.id] ?? []) {
        add(p);
      }
    }
    // The pass's own authored Parameter list (graph OR whole-pass).
    for (const p of pass.parameters) {
      add(p);
    }
  }

  return out;
}

/** The set of whole-pass code pass ids in a project (the passes that need scanning). */
export function wholePassIds(project: Project): string[] {
  return project.passes
    .filter((p) => p.source.kind === "wholePassCode")
    .map((p) => p.id);
}
