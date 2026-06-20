// Export substitution (#54, Decision Log #5: generated source is OUTPUT-ONLY).
//
// The Rust exporter (`export_preset`) throws `GraphPassUnsupported` for a graph
// pass — it can only write a whole-pass `.slang` verbatim. So at export time the
// FRONTEND substitutes each graph pass's GENERATED slang (from `compile_graph`,
// the same source the live preview ran) as a `PassSource::WholePassCode` in the
// Project it sends to `export_preset`. The exporter then writes it unchanged.
//
// This is the seam that makes #55's "the displayed source matches the exported
// per-pass .slang" hold: the bytes exported are exactly the bytes compiled +
// previewed. A graph pass that did NOT compile cleanly (no generated source) is
// reported as a blocker — an invalid pipeline cannot be exported.
//
// Pure: takes the project + the per-pass generated sources (the caller compiles
// first via the compile loop), so it is unit-testable with no Tauri.
import type { Pass } from "../bindings/Pass";
import type { Project } from "../bindings/Project";

/** The outcome of preparing a project for export. */
export type ExportSubstitution =
  | { ok: true; project: Project }
  | {
      ok: false;
      /** Pass ids whose graph produced no compiled source (so cannot be exported). */
      uncompiledPassIds: string[];
    };

/**
 * Build the export-ready Project by replacing every GRAPH pass with a whole-pass
 * code pass carrying its generated slang (from `sourcesByPassId`). Whole-pass code
 * passes are left untouched. Returns `ok:false` (with the offending pass ids) when
 * a graph pass has no generated source — an invalid pipeline is not exportable.
 *
 * The substitution is OUTPUT-ONLY: it never mutates the editor's document; it
 * returns a fresh Project to hand to `export_preset`.
 */
export function substituteGraphPasses(
  project: Project,
  sourcesByPassId: Record<string, string | null>,
): ExportSubstitution {
  const uncompiledPassIds: string[] = [];
  const passes: Pass[] = project.passes.map((pass) => {
    if (pass.source.kind !== "graph") {
      return pass;
    }
    const source = sourcesByPassId[pass.id];
    if (source == null) {
      uncompiledPassIds.push(pass.id);
      return pass;
    }
    return {
      ...pass,
      source: {
        kind: "wholePassCode",
        source,
        // A `.slang` filename the exporter uses for the written file basename; the
        // exporter derives one when null, so leave it null for generated passes.
        filename: null,
        opaque: true,
      },
    };
  });

  if (uncompiledPassIds.length > 0) {
    return { ok: false, uncompiledPassIds };
  }
  return { ok: true, project: { ...project, passes } };
}

/** Extract `passId → generated source` from a compile result's per-pass list. */
export function sourcesFromCompile(
  passes: Array<{ passId: string; source: string | null }>,
): Record<string, string | null> {
  const out: Record<string, string | null> = {};
  for (const p of passes) {
    out[p.passId] = p.source;
  }
  return out;
}
