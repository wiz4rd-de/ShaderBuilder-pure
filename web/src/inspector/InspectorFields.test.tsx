import { describe, expect, it } from "vitest";

import { reconcileVecText } from "./InspectorFields";

describe("reconcileVecText (#10) — vec text resync", () => {
  it("keeps an in-progress decimal that still parses to the stored value", () => {
    // User typed "0." in component x; live() wrote the parsed 0, which equals the
    // stored 0 — the store re-render must NOT clobber the lone trailing dot.
    const prev = ["0.", "0"];
    expect(reconcileVecText(prev, [0, 0])).toBe(prev); // same ref => no setText churn
    expect(reconcileVecText(prev, [0, 0])).toEqual(["0.", "0"]);
  });

  it("keeps a trailing-zero entry like '1.50' (parses equal, must survive)", () => {
    const prev = ["1.50", "2"];
    expect(reconcileVecText(prev, [1.5, 2])).toEqual(["1.50", "2"]);
  });

  it("resyncs when the stored value genuinely changed (undo/redo, external edit)", () => {
    // A real external change parses differently, so the text IS replaced.
    expect(reconcileVecText(["0.", "0"], [3, 4])).toEqual(["3", "4"]);
  });

  it("resyncs an empty / non-numeric component back to the stored value", () => {
    // A blank component does not parse to the stored 5, so it resyncs.
    expect(reconcileVecText(["", "0"], [5, 0])).toEqual(["5", "0"]);
  });
});
