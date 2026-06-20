import { beforeEach, describe, expect, it } from "vitest";

import type { Project } from "../bindings/Project";
import { useDocumentStore } from "./documentStore";
import { makeProject } from "./factories";
import { resetIdsForTest } from "./ids";

function store() {
  return useDocumentStore.getState();
}

beforeEach(() => {
  resetIdsForTest();
  store().reset();
});

describe("currentProjectPath / markSaved (#63)", () => {
  it("a fresh project is untitled (no path) and clean", () => {
    expect(store().currentProjectPath).toBeNull();
    expect(store().dirty).toBe(false);
  });

  it("loadProject records the file path and clears dirty", () => {
    const project: Project = makeProject("Loaded");
    store().loadProject(project, undefined, "/tmp/loaded.json");
    expect(store().currentProjectPath).toBe("/tmp/loaded.json");
    expect(store().dirty).toBe(false);
  });

  it("loadProject with no path clears the association (recovered/untitled)", () => {
    store().setCurrentProjectPath("/tmp/old.json");
    store().loadProject(makeProject("X"));
    expect(store().currentProjectPath).toBeNull();
  });

  it("markSaved clears dirty and optionally records the path", () => {
    store().addNode("placeholder", { x: 0, y: 0 });
    expect(store().dirty).toBe(true);
    store().markSaved("/tmp/saved.json");
    expect(store().dirty).toBe(false);
    expect(store().currentProjectPath).toBe("/tmp/saved.json");
  });

  it("reset clears both dirty and the path", () => {
    store().loadProject(makeProject("X"), undefined, "/tmp/x.json");
    store().addNode("placeholder", { x: 0, y: 0 });
    store().reset();
    expect(store().currentProjectPath).toBeNull();
    expect(store().dirty).toBe(false);
  });

  it("an edit after a load marks the document dirty again", () => {
    store().loadProject(makeProject("X"), undefined, "/tmp/x.json");
    expect(store().dirty).toBe(false);
    store().addNode("placeholder", { x: 0, y: 0 });
    expect(store().dirty).toBe(true);
    // The path association survives an edit.
    expect(store().currentProjectPath).toBe("/tmp/x.json");
  });
});
