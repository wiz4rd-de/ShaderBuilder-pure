// Parameter value reconciliation (#53). The slider panel keeps a live value map
// keyed by parameter NAME — the current knob positions — separate from the
// declarations (which change as the graph is edited). When the declaration set
// changes (a param added/removed/renamed) we must reconcile the value map:
//
//   * a name that PERSISTS keeps its current value (do not snap back to default);
//   * a name that DISAPPEARS is dropped (its UBO slot no longer exists);
//   * a name that APPEARS gets its declared default (unless a stale value lingers
//     — but a dropped name is fully forgotten, so a re-appearing name defaults).
//
// Pure + synchronous so it is unit-tested directly and reused by the hook.
import type { Parameter } from "../bindings/Parameter";

/** A value map: parameter name → current slider value. */
export type ValueMap = Readonly<Record<string, number>>;

/** Clamp `value` into the parameter's declared [min, max] (engine also clamps). */
export function clampToParam(param: Parameter, value: number): number {
  if (!Number.isFinite(value)) {
    return param.default;
  }
  const lo = Math.min(param.min, param.max);
  const hi = Math.max(param.min, param.max);
  return Math.min(hi, Math.max(lo, value));
}

/**
 * Reconcile a previous value map against the current declarations: keep persisting
 * names (clamped to their possibly-changed range), drop vanished names, seed new
 * names with their default. Returns a NEW map (referentially equal content does
 * NOT guarantee a new reference — callers compare by content if they care).
 */
export function reconcileValues(
  prev: ValueMap,
  params: ReadonlyArray<Parameter>,
): ValueMap {
  const next: Record<string, number> = {};
  for (const param of params) {
    const had = Object.prototype.hasOwnProperty.call(prev, param.name);
    const raw = had ? prev[param.name]! : param.default;
    next[param.name] = clampToParam(param, raw);
  }
  return next;
}

/** Whether two value maps hold the same names and values (shallow numeric compare). */
export function valueMapsEqual(a: ValueMap, b: ValueMap): boolean {
  const ak = Object.keys(a);
  const bk = Object.keys(b);
  if (ak.length !== bk.length) {
    return false;
  }
  for (const k of ak) {
    if (!Object.prototype.hasOwnProperty.call(b, k) || a[k] !== b[k]) {
      return false;
    }
  }
  return true;
}
