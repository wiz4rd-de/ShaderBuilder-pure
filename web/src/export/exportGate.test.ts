import { describe, expect, it } from "vitest";

import type { ExportBlocker } from "../bindings/ExportBlocker";
import type { ExportResult } from "../bindings/ExportResult";
import type { Project } from "../bindings/Project";
import { EMPTY_PROJECT } from "../model";
import {
  blockerToReason,
  blockingReasons,
  exportErrorMessage,
  outcomeErrorMessage,
  successMessage,
  summarizeProject,
} from "./exportGate";

describe("summarizeProject", () => {
  it("counts passes, parameters, and LUTs", () => {
    const project: Project = {
      ...EMPTY_PROJECT,
      passes: [
        { ...(EMPTY_PROJECT.passes[0] ?? ({} as never)), id: "a" },
        { ...(EMPTY_PROJECT.passes[0] ?? ({} as never)), id: "b" },
      ],
      parameters: [
        { name: "P", label: "P", default: 0, min: 0, max: 1, step: 0.1 },
      ],
      luts: [
        { name: "L", path: "/l.png", filterLinear: null, wrapMode: null, mipmap: null },
      ],
    };
    expect(summarizeProject(project)).toEqual({
      passCount: 2,
      parameterCount: 1,
      lutCount: 1,
    });
  });
});

describe("blockerToReason", () => {
  it("renders each blocker variant with its pass link", () => {
    expect(blockerToReason({ kind: "noPasses" }).passId).toBeNull();
    const graph = blockerToReason({
      kind: "uncompiledGraphPass",
      passId: "g0",
      passName: "Graph",
    });
    expect(graph.passId).toBe("g0");
    expect(graph.message).toContain("Graph");
    const empty = blockerToReason({
      kind: "emptyPassSource",
      passId: "e0",
      passName: "Empty",
    });
    expect(empty.passId).toBe("e0");
  });
});

describe("blockingReasons", () => {
  it("is empty for a valid pipeline with no blockers", () => {
    expect(blockingReasons(true, [])).toEqual([]);
  });

  it("flags a not-yet-compiled pipeline", () => {
    const reasons = blockingReasons(null, []);
    expect(reasons).toHaveLength(1);
    expect(reasons[0]!.message).toContain("not compiled");
  });

  it("adds a pipeline-invalid reason when no structural blocker covers it", () => {
    const reasons = blockingReasons(false, []);
    expect(reasons.some((r) => r.message.includes("not renderable"))).toBe(true);
  });

  it("does not duplicate when the gate already reports an uncompiled graph pass", () => {
    const blockers: ExportBlocker[] = [
      { kind: "uncompiledGraphPass", passId: "g", passName: "G" },
    ];
    const reasons = blockingReasons(false, blockers);
    // Only the structural blocker — no extra generic "not renderable" line.
    expect(reasons).toHaveLength(1);
    expect(reasons[0]!.passId).toBe("g");
  });
});

describe("outcome + error messages", () => {
  it("describes a notRenderable outcome with the offending passes", () => {
    const msg = outcomeErrorMessage({ kind: "notRenderable", uncompiledPassIds: ["a", "b"] });
    expect(msg).toContain("a, b");
    expect(msg).toContain("No files were written");
  });

  it("describes an io write failure", () => {
    expect(exportErrorMessage({ kind: "io", message: "permission denied" })).toContain(
      "permission denied",
    );
  });

  it("flags a post-substitution graph pass as an internal error", () => {
    const msg = exportErrorMessage({ kind: "graphPassUnsupported", passId: "p" });
    expect(msg).toContain("Internal export error");
  });

  it("names the bundle path + file counts on success", () => {
    const result: ExportResult = {
      presetPath: "/out/preset.slangp",
      passFiles: ["a.slang", "b.slang"],
      textureFiles: ["t.png"],
      warnings: [],
    };
    const msg = successMessage(result);
    expect(msg).toContain("/out/preset.slangp");
    expect(msg).toContain("2 pass file(s)");
    expect(msg).toContain("1 LUT(s)");
  });
});
