import { describe, expect, it } from "vitest";

import type { NodeOp } from "../bindings/NodeOp";
import {
  ALL_DESCRIPTORS,
  descriptorsByCategory,
  getDescriptor,
  hasDescriptor,
  listDescriptors,
  nonEmptyCategories,
  requireDescriptor,
} from "./registry";
import { NodeLoweringError } from "./types";

describe("node-descriptor registry", () => {
  it("registers every #49 boundary node kind", () => {
    const kinds = [
      // inputs / samplers
      "source",
      "original",
      "originalHistory",
      "passOutput",
      "passFeedback",
      "lut",
      // coordinates / UV
      "texcoord",
      "uvOffset",
      "uvRotate",
      "uvWarp",
      "uvCurvature",
      // constants / params / builtins
      "const",
      "param",
      "builtin.sourceSize",
      "builtin.originalSize",
      "builtin.outputSize",
      "builtin.finalViewportSize",
      "builtin.frameCount",
      "builtin.frameDirection",
      // math + vector (#50)
      "math",
      "swizzle",
      "split",
      "combine",
      // output
      "output",
    ];
    for (const kind of kinds) {
      expect(hasDescriptor(kind), kind).toBe(true);
    }
  });

  it("keys descriptors uniquely by kind", () => {
    const kinds = ALL_DESCRIPTORS.map((d) => d.kind);
    expect(new Set(kinds).size).toBe(kinds.length);
  });

  it("getDescriptor/requireDescriptor resolve and reject correctly", () => {
    expect(getDescriptor("source")?.kind).toBe("source");
    expect(getDescriptor("nope")).toBeUndefined();
    expect(() => requireDescriptor("nope")).toThrow(/unknown node kind/);
  });

  it("every descriptor's defaultData lowers to a NodeOp without throwing", () => {
    // `param` and `lut` require a user-supplied name (an empty default is an
    // intentional lowering error until the inspector fills it in) — exclude them.
    const requiresUserData = new Set(["param", "lut"]);
    for (const d of listDescriptors()) {
      if (requiresUserData.has(d.kind)) {
        continue;
      }
      const op: NodeOp = d.toNodeOp(d.defaultData());
      expect(op.kind).toBeTruthy();
    }
  });

  it("every descriptor exposes consistent port arrays for its defaults", () => {
    for (const d of listDescriptors()) {
      const data = d.defaultData();
      const inputs = d.inputs(data);
      const outputs = d.outputs(data);
      expect(Array.isArray(inputs)).toBe(true);
      expect(Array.isArray(outputs)).toBe(true);
      // Port names within a node are unique per side.
      expect(new Set(inputs.map((p) => p.name)).size).toBe(inputs.length);
      expect(new Set(outputs.map((p) => p.name)).size).toBe(outputs.length);
    }
  });

  it("every customSnippet descriptor's PortSpec names equal its lowered PortDecl names", () => {
    // The checker matches CustomSnippet edges STRICTLY by declared PortDecl name,
    // and the React-Flow handle id = the canvas PortSpec name (graphToIr copies
    // edge.sourcePort/targetPort verbatim). So a descriptor whose canvas PortSpec
    // names differ from the names its `toNodeOp` snippet emits makes EVERY edge
    // through that node an unknownPort/danglingInput. Guard the whole registry.
    for (const d of listDescriptors()) {
      const data = d.defaultData();
      let op: NodeOp;
      try {
        op = d.toNodeOp(data);
      } catch {
        // `param`/`lut` intentionally throw on empty default data — skip them.
        continue;
      }
      if (op.kind !== "customSnippet") continue;
      const inputNames = d.inputs(data).map((p) => p.name);
      const outputNames = d.outputs(data).map((p) => p.name);
      expect(op.inputs.map((p) => p.name), `${d.kind} inputs`).toEqual(inputNames);
      expect(op.outputs.map((p) => p.name), `${d.kind} outputs`).toEqual(outputNames);
    }
  });

  it("only non-empty categories are listed, in canonical order", () => {
    const cats = nonEmptyCategories();
    expect(cats).toContain("input");
    expect(cats).toContain("coordinate");
    expect(cats).toContain("constant");
    expect(cats).toContain("parameter");
    expect(cats).toContain("builtin");
    expect(cats).toContain("output");
    // No empty category slips in.
    for (const c of cats) {
      expect(descriptorsByCategory(c).length).toBeGreaterThan(0);
    }
  });
});

describe("sampler descriptors", () => {
  it("all samplers expose a required vec2 coord input + vec4 out", () => {
    for (const kind of ["source", "original", "originalHistory", "passOutput", "passFeedback", "lut"]) {
      const d = requireDescriptor(kind);
      const data = d.defaultData();
      const inputs = d.inputs(data);
      const outputs = d.outputs(data);
      expect(inputs).toEqual([{ name: "coord", type: "vec2", label: "UV" }]);
      expect(outputs.map((p) => ({ name: p.name, type: p.type }))).toEqual([
        { name: "out", type: "vec4" },
      ]);
    }
  });

  it("source/original lower to fixed TextureSources", () => {
    expect(requireDescriptor("source").toNodeOp({})).toEqual({
      kind: "sample",
      texture: { kind: "source" },
    });
    expect(requireDescriptor("original").toNodeOp({})).toEqual({
      kind: "sample",
      texture: { kind: "original" },
    });
  });

  it("indexed samplers carry their data.index", () => {
    expect(requireDescriptor("originalHistory").toNodeOp({ index: 3 })).toEqual({
      kind: "sample",
      texture: { kind: "originalHistory", index: 3 },
    });
    expect(requireDescriptor("passOutput").toNodeOp({ index: 1 })).toEqual({
      kind: "sample",
      texture: { kind: "passOutput", index: 1 },
    });
    expect(requireDescriptor("passFeedback").toNodeOp({ index: 2 })).toEqual({
      kind: "sample",
      texture: { kind: "passFeedback", index: 2 },
    });
  });

  it("indexed samplers default a missing index to 0", () => {
    expect(requireDescriptor("passOutput").toNodeOp({})).toEqual({
      kind: "sample",
      texture: { kind: "passOutput", index: 0 },
    });
  });

  it("a dangling/negative index throws rather than silently clamping to 0", () => {
    // DANGLING_INDEX (-1) is the sentinel removePass writes when the referenced
    // pass is deleted (pipeline/passOps.ts). TextureSource.index is a Rust u32,
    // so a negative index must NOT round-trip — and must NOT be clamped to 0
    // (which would re-point the sampler at PassOutput0 and mis-wire the chain).
    for (const kind of ["passOutput", "passFeedback", "originalHistory"] as const) {
      expect(() => requireDescriptor(kind).toNodeOp({ index: -1 })).toThrow(
        /removed pass/,
      );
      expect(() => requireDescriptor(kind).toNodeOp({ index: -4 })).toThrow(
        NodeLoweringError,
      );
    }
  });

  it("LUT lowers to a named TextureSource + reports its LUT name", () => {
    const lut = requireDescriptor("lut");
    expect(lut.toNodeOp({ name: "overlay" })).toEqual({
      kind: "sample",
      texture: { kind: "lut", name: "overlay" },
    });
    expect(lut.toLutName?.({ name: "overlay" })).toBe("overlay");
    expect(lut.toLutName?.({ name: "" })).toBeNull();
  });

  it("LUT with no name throws a lowering error", () => {
    expect(() => requireDescriptor("lut").toNodeOp({})).toThrow(/name/);
  });
});

describe("coordinate descriptors", () => {
  it("texcoord lowers to a CustomSnippet reading vTexCoord → vec2 uv", () => {
    const op = requireDescriptor("texcoord").toNodeOp({});
    expect(op.kind).toBe("customSnippet");
    if (op.kind !== "customSnippet") return;
    expect(op.inputs).toEqual([]);
    expect(op.outputs).toEqual([{ name: "uv", type: "vec2" }]);
    expect(op.body).toContain("vTexCoord");
  });

  it("texcoord exposes no inputs and a vec2 uv output", () => {
    const d = requireDescriptor("texcoord");
    expect(d.inputs({})).toEqual([]);
    expect(d.outputs({})).toEqual([{ name: "uv", type: "vec2", label: "UV" }]);
  });

  it("UV transforms take a vec2 uv input and yield a vec2 uv output", () => {
    for (const kind of ["uvOffset", "uvRotate", "uvWarp", "uvCurvature"]) {
      const d = requireDescriptor(kind);
      const data = d.defaultData();
      expect(d.inputs(data).map((p) => p.type)).toEqual(["vec2"]);
      expect(d.outputs(data).map((p) => p.type)).toEqual(["vec2"]);
      const op = d.toNodeOp(data);
      expect(op.kind).toBe("customSnippet");
      if (op.kind !== "customSnippet") continue;
      expect(op.inputs.map((p) => p.type)).toEqual(["vec2"]);
      expect(op.outputs.map((p) => p.type)).toEqual(["vec2"]);
    }
  });

  it("UV offset bakes its data into the snippet body", () => {
    const op = requireDescriptor("uvOffset").toNodeOp({ x: 0.25, y: -0.5 });
    if (op.kind !== "customSnippet") throw new Error("expected customSnippet");
    expect(op.body).toContain("0.25");
    expect(op.body).toContain("-0.5");
  });
});

describe("const descriptor", () => {
  it("defaults to a float const", () => {
    expect(requireDescriptor("const").toNodeOp({ constType: "float", value: 0 })).toEqual({
      kind: "const",
      value: { kind: "float", value: 0 },
    });
  });

  it("lowers each variant with the right components + output type", () => {
    const d = requireDescriptor("const");
    expect(d.toNodeOp({ constType: "vec3", value: [1, 2, 3] })).toEqual({
      kind: "const",
      value: { kind: "vec3", value: [1, 2, 3] },
    });
    expect(d.outputs({ constType: "vec3" }).map((p) => p.type)).toEqual(["vec3"]);
    expect(d.toNodeOp({ constType: "int", value: 4.9 })).toEqual({
      kind: "const",
      value: { kind: "int", value: 4 },
    });
    expect(d.outputs({ constType: "int" }).map((p) => p.type)).toEqual(["int"]);
    expect(d.toNodeOp({ constType: "bool", value: true })).toEqual({
      kind: "const",
      value: { kind: "bool", value: true },
    });
    expect(d.outputs({ constType: "bool" }).map((p) => p.type)).toEqual(["bool"]);
  });

  it("pads a short vec value with the default components", () => {
    expect(requireDescriptor("const").toNodeOp({ constType: "vec4", value: [1, 2] })).toEqual({
      kind: "const",
      value: { kind: "vec4", value: [1, 2, 0, 0] },
    });
  });
});

describe("param descriptor", () => {
  it("lowers to Param{name} from data.name (not the node id)", () => {
    expect(requireDescriptor("param").toNodeOp({ name: "GAMMA" })).toEqual({
      kind: "param",
      name: "GAMMA",
    });
  });

  it("throws when the pragma name is missing", () => {
    expect(() => requireDescriptor("param").toNodeOp({})).toThrow(/name/);
  });

  it("contributes a pass Parameter capturing name/label/default/range/step", () => {
    const d = requireDescriptor("param");
    const param = d.toParameter?.({
      name: "GAMMA",
      label: "Gamma",
      default: 1.0,
      min: 0.1,
      max: 3.0,
      step: 0.05,
    });
    expect(param).toEqual({
      name: "GAMMA",
      label: "Gamma",
      default: 1.0,
      min: 0.1,
      max: 3.0,
      step: 0.05,
    });
  });

  it("contributes no Parameter when unnamed", () => {
    expect(requireDescriptor("param").toParameter?.({ name: "" })).toBeNull();
  });

  it("outputs a single float port", () => {
    expect(requireDescriptor("param").outputs({}).map((p) => p.type)).toEqual(["float"]);
  });
});

describe("builtin descriptors", () => {
  it("each builtin lowers to its semantic with the documented output type", () => {
    const cases: Array<[string, string, string]> = [
      ["builtin.sourceSize", "sourceSize", "vec4"],
      ["builtin.originalSize", "originalSize", "vec4"],
      ["builtin.outputSize", "outputSize", "vec4"],
      ["builtin.finalViewportSize", "finalViewportSize", "vec4"],
      ["builtin.frameCount", "frameCount", "int"],
      ["builtin.frameDirection", "frameDirection", "int"],
    ];
    for (const [kind, semantic, ty] of cases) {
      const d = requireDescriptor(kind);
      expect(d.toNodeOp({})).toEqual({ kind: "builtin", semantic });
      expect(d.outputs({}).map((p) => p.type)).toEqual([ty]);
    }
  });

  it("does not register an MVP builtin node (no fragment value port)", () => {
    expect(hasDescriptor("builtin.mvp")).toBe(false);
  });
});

describe("output descriptor", () => {
  it("requires a vec4 color input and has no output", () => {
    const d = requireDescriptor("output");
    expect(d.inputs({})).toEqual([{ name: "color", type: "vec4", label: "color" }]);
    expect(d.outputs({})).toEqual([]);
    expect(d.toNodeOp({})).toEqual({ kind: "output" });
  });
});
