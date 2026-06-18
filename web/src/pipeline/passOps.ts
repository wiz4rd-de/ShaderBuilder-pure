// Pure pass-array operations for the pipeline view (#46) — add / remove / reorder
// passes, keeping the document's index-based texture references consistent.
//
// REORDER / REMOVE INDEX-REMAP POLICY (documented + tested)
// ---------------------------------------------------------
// A pass's per-pass graph can carry sampler nodes that bind an EARLIER pass BY
// INDEX: `PassOutputN` (TextureSource.passOutput{index}) and `PassFeedbackN`
// (TextureSource.passFeedback{index}). When passes are reordered or pruned, the
// authoritative pass index of a referenced pass changes, so those stored indices
// would otherwise silently mis-wire the chain (issue GOTCHA).
//
// Policy: we treat the pass ORDER as authoritative (Project.passes order = the
// .slangp pass index) and REMAP every passOutput/passFeedback index through the
// permutation produced by the reorder/remove, so each sampler keeps pointing at
// the SAME pass it referenced before. When a remove deletes the very pass a
// sampler referenced, that index can no longer resolve: we clamp it to -1 (an
// out-of-range sentinel) so the downstream compile surfaces it as an error
// rather than silently re-pointing at a different pass. `project.feedbackPass`
// (the global feedback pass index) is remapped the same way.
//
// Alias-based references (TextureSource carries no alias variant in the frozen
// IR, so graph passes reference earlier passes only by index) need no remap.
import type { Pass } from "../bindings/Pass";
import type { Project } from "../bindings/Project";

/** Sentinel stored for an index reference whose target pass was removed. */
export const DANGLING_INDEX = -1;

/**
 * Remap a single old pass index through `oldToNew` (old index → new index, or
 * `undefined` when the pass was removed). Returns the new index, or
 * `DANGLING_INDEX` when the referenced pass no longer exists.
 */
function remapIndex(index: number, oldToNew: ReadonlyMap<number, number>): number {
  const next = oldToNew.get(index);
  return next === undefined ? DANGLING_INDEX : next;
}

/**
 * Rewrite the index-based texture references inside one pass's graph through the
 * permutation `oldToNew`. Returns a NEW pass (shallow-cloned along the touched
 * path) only if anything changed; otherwise returns the same pass reference so
 * untouched passes stay referentially stable.
 */
function remapPassIndices(pass: Pass, oldToNew: ReadonlyMap<number, number>): Pass {
  if (pass.source.kind !== "graph") {
    return pass;
  }
  let changed = false;
  const nodes = pass.source.graph.nodes.map((node) => {
    // Sampler nodes lower to NodeOp::sample{texture}, but in the SKELETAL graph
    // the index lives in node.data.index for indexed samplers (see
    // descriptors/samplers.ts). Remap by node.kind, not by lowered op.
    if (node.kind !== "passOutput" && node.kind !== "passFeedback") {
      return node;
    }
    const raw = (node.data as { index?: unknown }).index;
    const index = typeof raw === "number" ? raw : 0;
    const next = remapIndex(index, oldToNew);
    if (next === index) {
      return node;
    }
    changed = true;
    return { ...node, data: { ...node.data, index: next } };
  });
  if (!changed) {
    return pass;
  }
  return { ...pass, source: { ...pass.source, graph: { ...pass.source.graph, nodes } } };
}

/**
 * Apply a pass permutation to a whole project: reorder `passes`, remap every
 * pass's index-based texture references, and remap `feedbackPass`.
 *
 * @param project    the source project (not mutated).
 * @param nextPasses the passes in their NEW order.
 * @param oldToNew   old pass index → new pass index (omit removed passes).
 */
function applyPermutation(
  project: Project,
  nextPasses: Pass[],
  oldToNew: ReadonlyMap<number, number>,
): Project {
  const remapped = nextPasses.map((p) => remapPassIndices(p, oldToNew));
  const feedbackPass =
    project.feedbackPass === null ? null : remapIndex(project.feedbackPass, oldToNew);
  return { ...project, passes: remapped, feedbackPass };
}

/** Append a fresh pass to the end of the pipeline (no index disturbance). */
export function addPass(project: Project, pass: Pass): Project {
  return { ...project, passes: [...project.passes, pass] };
}

/**
 * Remove the pass with `passId`. Remaining passes shift down; their index
 * references are remapped, and references to the removed pass become
 * `DANGLING_INDEX`. Removing the last remaining pass is rejected (returns the
 * project unchanged) — a pipeline always has at least one pass.
 */
export function removePass(project: Project, passId: string): Project {
  if (project.passes.length <= 1) {
    return project;
  }
  const removedIndex = project.passes.findIndex((p) => p.id === passId);
  if (removedIndex < 0) {
    return project;
  }
  const nextPasses = project.passes.filter((p) => p.id !== passId);
  const oldToNew = new Map<number, number>();
  let newIndex = 0;
  project.passes.forEach((_, oldIndex) => {
    if (oldIndex === removedIndex) {
      return; // removed: no mapping entry → references become DANGLING_INDEX
    }
    oldToNew.set(oldIndex, newIndex);
    newIndex += 1;
  });
  return applyPermutation(project, nextPasses, oldToNew);
}

/**
 * Move the pass at `from` to `to` (both 0-based indices into Project.passes).
 * Index references are remapped so each sampler keeps pointing at the same pass.
 * Out-of-range / no-op moves return the project unchanged.
 */
export function reorderPass(project: Project, from: number, to: number): Project {
  const n = project.passes.length;
  if (from < 0 || from >= n || to < 0 || to >= n || from === to) {
    return project;
  }
  const nextPasses = [...project.passes];
  const [moved] = nextPasses.splice(from, 1);
  nextPasses.splice(to, 0, moved!);
  // Build old index → new index from the resulting order (passes keep ids).
  const idToNew = new Map(nextPasses.map((p, i) => [p.id, i] as const));
  const oldToNew = new Map<number, number>();
  project.passes.forEach((p, oldIndex) => {
    oldToNew.set(oldIndex, idToNew.get(p.id)!);
  });
  return applyPermutation(project, nextPasses, oldToNew);
}
