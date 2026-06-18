import { describe, expect, it } from "vitest";

import type { Parameter } from "../bindings/Parameter";
import { clampToParam, reconcileValues, valueMapsEqual } from "./reconcile";

function param(name: string, over: Partial<Parameter> = {}): Parameter {
  return { name, label: name, default: 0.5, min: 0, max: 1, step: 0.01, ...over };
}

describe("clampToParam", () => {
  it("clamps into [min,max] and handles reversed ranges", () => {
    expect(clampToParam(param("a", { min: 0, max: 1 }), 2)).toBe(1);
    expect(clampToParam(param("a", { min: 0, max: 1 }), -1)).toBe(0);
    expect(clampToParam(param("a", { min: 1, max: 0 }), 0.5)).toBe(0.5);
  });

  it("falls back to default on a non-finite value", () => {
    expect(clampToParam(param("a", { default: 0.3 }), NaN)).toBe(0.3);
  });
});

describe("reconcileValues", () => {
  it("seeds new names with their default", () => {
    const next = reconcileValues({}, [param("g", { default: 0.7 })]);
    expect(next).toEqual({ g: 0.7 });
  });

  it("preserves a persisting name's current value", () => {
    const next = reconcileValues({ g: 0.2 }, [param("g", { default: 0.7 })]);
    expect(next.g).toBe(0.2);
  });

  it("drops a vanished name", () => {
    const next = reconcileValues({ g: 0.2, old: 0.9 }, [param("g")]);
    expect(next).toEqual({ g: 0.2 });
    expect("old" in next).toBe(false);
  });

  it("re-clamps a persisting value when the range shrinks", () => {
    const next = reconcileValues({ g: 0.9 }, [param("g", { min: 0, max: 0.5 })]);
    expect(next.g).toBe(0.5);
  });

  it("a re-appearing name (after being dropped) defaults", () => {
    // Round 1: g + h present.
    const r1 = reconcileValues({}, [param("g"), param("h")]);
    // Round 2: h dropped.
    const r2 = reconcileValues(r1, [param("g")]);
    // Round 3: h re-appears → it was forgotten, so it defaults.
    const r3 = reconcileValues(r2, [param("g"), param("h", { default: 0.4 })]);
    expect(r3.h).toBe(0.4);
  });
});

describe("valueMapsEqual", () => {
  it("compares names and numeric values", () => {
    expect(valueMapsEqual({ a: 1 }, { a: 1 })).toBe(true);
    expect(valueMapsEqual({ a: 1 }, { a: 2 })).toBe(false);
    expect(valueMapsEqual({ a: 1 }, { a: 1, b: 2 })).toBe(false);
    expect(valueMapsEqual({ a: 1, b: 2 }, { a: 1 })).toBe(false);
  });
});
