import { describe, expect, it } from "vitest";

import type { Node } from "../bindings/Node";
import type { Parameter } from "../bindings/Parameter";
import type { Project } from "../bindings/Project";
import { makeProject } from "../store/factories";
import { collectParameters, wholePassIds } from "./collectParameters";

function param(name: string, over: Partial<Parameter> = {}): Parameter {
  return { name, label: name, default: 0, min: 0, max: 1, step: 0.01, ...over };
}

/** A project with one graph pass containing the given nodes. */
function graphProject(nodes: Node[]): Project {
  const p = makeProject();
  const pass = p.passes[0]!;
  if (pass.source.kind === "graph") {
    pass.source.graph.nodes = nodes;
  }
  return p;
}

describe("collectParameters", () => {
  it("collects Param-node declarations from a graph pass", () => {
    const project = graphProject([
      {
        id: "n1",
        kind: "param",
        position: { x: 0, y: 0 },
        data: { name: "gamma", label: "Gamma", default: 1, min: 0, max: 3, step: 0.05 },
      },
    ]);
    const params = collectParameters(project);
    expect(params).toHaveLength(1);
    expect(params[0]).toMatchObject({ name: "gamma", label: "Gamma", default: 1, max: 3 });
  });

  it("de-duplicates by name across passes (first wins) — global by id", () => {
    const project = makeProject();
    // Two graph passes both declaring `bright`.
    const second = JSON.parse(JSON.stringify(project.passes[0]!));
    second.id = "pass-2";
    project.passes.push(second);
    for (const pass of project.passes) {
      if (pass.source.kind === "graph") {
        pass.source.graph.nodes = [
          {
            id: `${pass.id}-p`,
            kind: "param",
            position: { x: 0, y: 0 },
            data: { name: "bright", label: pass.id, default: 0.5, min: 0, max: 1, step: 0.01 },
          },
        ];
      }
    }
    const params = collectParameters(project);
    expect(params).toHaveLength(1);
    expect(params[0]!.name).toBe("bright");
    // First declaration's label wins.
    expect(params[0]!.label).toBe(project.passes[0]!.id);
  });

  it("project-level parameters take precedence over pass declarations", () => {
    const project = graphProject([
      {
        id: "n1",
        kind: "param",
        position: { x: 0, y: 0 },
        data: { name: "gamma", label: "node label", default: 9, min: 0, max: 9, step: 1 },
      },
    ]);
    project.parameters = [param("gamma", { label: "project label", default: 1, max: 3 })];
    const params = collectParameters(project);
    expect(params).toHaveLength(1);
    expect(params[0]).toMatchObject({ label: "project label", default: 1, max: 3 });
  });

  it("includes whole-pass scanned parameters keyed by pass id", () => {
    const project = makeProject();
    const pass = project.passes[0]!;
    pass.source = { kind: "wholePassCode", source: "// code", filename: null, opaque: true };
    const params = collectParameters(project, {
      [pass.id]: [param("contrast", { default: 1, max: 2 })],
    });
    expect(params.map((p) => p.name)).toEqual(["contrast"]);
  });

  it("ignores unnamed params and missing scan entries", () => {
    const project = makeProject();
    const pass = project.passes[0]!;
    pass.source = { kind: "wholePassCode", source: "", filename: null, opaque: true };
    // No scan entry for this pass → contributes nothing, no throw.
    expect(collectParameters(project)).toEqual([]);
    expect(collectParameters(project, { [pass.id]: [param("", {})] })).toEqual([]);
  });

  it("merges a pass's authored Parameter list", () => {
    const project = makeProject();
    project.passes[0]!.parameters = [param("sat", { default: 1, max: 2 })];
    const params = collectParameters(project);
    expect(params.map((p) => p.name)).toEqual(["sat"]);
  });
});

describe("wholePassIds", () => {
  it("returns only the whole-pass code pass ids", () => {
    const project = makeProject();
    expect(wholePassIds(project)).toEqual([]);
    project.passes[0]!.source = {
      kind: "wholePassCode",
      source: "",
      filename: null,
      opaque: true,
    };
    expect(wholePassIds(project)).toEqual([project.passes[0]!.id]);
  });
});
