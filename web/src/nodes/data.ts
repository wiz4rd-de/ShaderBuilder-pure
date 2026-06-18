// Small typed accessors over a node's free-form `Record<string, unknown>` data.
// Descriptors read their config through these so a malformed/absent value falls
// back to a sane default rather than crashing graphToIr; `requireString` /
// `requireFiniteNumber` throw a NodeLoweringError when a value is genuinely
// missing where the op cannot be formed without it.
import type { NodeData } from "./types";
import { NodeLoweringError } from "./types";

/** Read a string at `key`, or `fallback` when absent/not-a-string. */
export function readString(data: NodeData, key: string, fallback: string): string {
  const v = data[key];
  return typeof v === "string" ? v : fallback;
}

/** Read a finite number at `key`, or `fallback` when absent/not-a-finite-number. */
export function readNumber(data: NodeData, key: string, fallback: number): number {
  const v = data[key];
  return typeof v === "number" && Number.isFinite(v) ? v : fallback;
}

/** Read an integer at `key` (truncating a finite number), or `fallback`. */
export function readInteger(data: NodeData, key: string, fallback: number): number {
  const v = data[key];
  return typeof v === "number" && Number.isFinite(v) ? Math.trunc(v) : fallback;
}

/** Read a boolean at `key`, or `fallback` when absent/not-a-boolean. */
export function readBoolean(data: NodeData, key: string, fallback: boolean): boolean {
  const v = data[key];
  return typeof v === "boolean" ? v : fallback;
}

/**
 * Read a fixed-length numeric tuple at `key`, padding/truncating to `len` and
 * substituting `fallback[i]` for any non-finite/absent component.
 */
export function readNumberTuple(data: NodeData, key: string, fallback: number[]): number[] {
  const v = data[key];
  const arr = Array.isArray(v) ? v : [];
  return fallback.map((fb, i) => {
    const c = arr[i];
    return typeof c === "number" && Number.isFinite(c) ? c : fb;
  });
}

/** Require a non-empty string at `key`, throwing a NodeLoweringError otherwise. */
export function requireString(kind: string, data: NodeData, key: string): string {
  const v = data[key];
  if (typeof v !== "string" || v.length === 0) {
    throw new NodeLoweringError(kind, `missing required string data.${key}`);
  }
  return v;
}
