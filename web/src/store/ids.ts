// Id generation for editor entities (nodes, edges, passes).
//
// Ids must be unique within a document and stable across serialization. We use
// a short monotonic prefix-counter rather than UUIDs so generated ids stay
// readable in diagnostics and tests, and remain unique even when many are
// minted in the same millisecond (a wall-clock/random scheme can collide under
// the synchronous bursts that copy/paste and duplicate produce).

let counter = 0;

/** Mint a fresh unique id with the given prefix, e.g. `node-12`, `edge-3`. */
export function nextId(prefix: string): string {
  counter += 1;
  return `${prefix}-${counter}`;
}

/**
 * Reset the id counter. Test-only — lets a test assert on deterministic ids.
 * Never call this from app code: it can mint ids that collide with existing
 * ones in a loaded document.
 */
export function resetIdsForTest(): void {
  counter = 0;
}
