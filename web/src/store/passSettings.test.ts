import { beforeEach, describe, expect, it } from "vitest";

import { useDocumentStore } from "./documentStore";
import { resetIdsForTest } from "./ids";

// Headless tests for the #48 pass-settings + feedback-pass store actions.
function store() {
  return useDocumentStore.getState();
}

function activePass() {
  const s = store();
  return s.project.passes.find((p) => p.id === s.activePassId)!;
}

beforeEach(() => {
  resetIdsForTest();
  store().reset();
});

describe("updatePassSettings", () => {
  it("merges a patch into Pass.settings as one undoable edit", () => {
    const id = store().activePassId;
    store().updatePassSettings(id, { filterLinear: true });
    expect(activePass().settings.filterLinear).toBe(true);
    expect(store().canUndo()).toBe(true);
    expect(store().dirty).toBe(true);

    store().undo();
    expect(activePass().settings.filterLinear).toBe(null);
  });

  it("replaces a ScaleAxis wholesale", () => {
    const id = store().activePassId;
    store().updatePassSettings(id, { scaleX: { scaleType: "absolute", scale: 256 } });
    expect(activePass().settings.scaleX).toEqual({ scaleType: "absolute", scale: 256 });
    // The other axis is untouched.
    expect(activePass().settings.scaleY).toEqual({ scaleType: null, scale: null });
  });

  it("is a no-op (no history entry) when nothing changes", () => {
    const id = store().activePassId;
    store().updatePassSettings(id, { scaleX: { scaleType: null, scale: null } });
    expect(store().canUndo()).toBe(false);
  });

  it("ignores undefined patch values but applies explicit null", () => {
    const id = store().activePassId;
    store().updatePassSettings(id, { alias: "blur" });
    store().updatePassSettings(id, { alias: undefined, filterLinear: false });
    expect(activePass().settings.alias).toBe("blur");
    expect(activePass().settings.filterLinear).toBe(false);
    store().updatePassSettings(id, { alias: null });
    expect(activePass().settings.alias).toBe(null);
  });

  it("does nothing for an unknown pass id", () => {
    store().updatePassSettings("nope", { filterLinear: true });
    expect(store().canUndo()).toBe(false);
  });
});

describe("setFeedbackPass", () => {
  it("sets and clears the project feedback pass index undoably", () => {
    store().setFeedbackPass(0);
    expect(store().project.feedbackPass).toBe(0);
    store().setFeedbackPass(null);
    expect(store().project.feedbackPass).toBe(null);
    store().undo();
    expect(store().project.feedbackPass).toBe(0);
  });

  it("is a no-op when unchanged", () => {
    store().setFeedbackPass(null); // already null
    expect(store().canUndo()).toBe(false);
  });
});
