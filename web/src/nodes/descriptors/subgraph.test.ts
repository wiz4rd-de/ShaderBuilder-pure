import { describe, expect, it } from "vitest";

import type { BoundaryPort } from "../../bindings/BoundaryPort";
import type { Subgraph } from "../../bindings/Subgraph";
import { NodeLoweringError } from "../types";
import { subgraphDescriptor } from "./subgraph";

function sub(boundaryPorts: BoundaryPort[], name = "S"): Subgraph {
  return { id: "s", name, nodes: [], edges: [], boundaryPorts };
}

describe("subgraph descriptor (#57)", () => {
  it("derives in/out ports from data.boundaryPorts, carrying BoundaryPort.ty", () => {
    const data = sub([
      { name: "coordIn", ty: "vec2", direction: "in", interiorNode: "x", interiorPort: "coord" },
      { name: "colorOut", ty: "vec4", direction: "out", interiorNode: "x", interiorPort: "out" },
      { name: "amount", ty: "float", direction: "in", interiorNode: "y", interiorPort: "k" },
    ]) as unknown as Record<string, unknown>;

    const inputs = subgraphDescriptor.inputs(data);
    const outputs = subgraphDescriptor.outputs(data);

    expect(inputs).toEqual([
      { name: "coordIn", type: "vec2" },
      { name: "amount", type: "float" },
    ]);
    expect(outputs).toEqual([{ name: "colorOut", type: "vec4" }]);
  });

  it("exposes the subgraph name through the data-derived title", () => {
    const data = sub([], "MyGroup") as unknown as Record<string, unknown>;
    expect(subgraphDescriptor.title?.(data)).toBe("MyGroup");
  });

  it("has an editable `name` text field in the inspector", () => {
    const fields = subgraphDescriptor.inspector({});
    expect(fields).toEqual([{ key: "name", label: "Name", kind: "text" }]);
  });

  it("toNodeOp throws — a subgraph must be inlined before lowering", () => {
    expect(() => subgraphDescriptor.toNodeOp({})).toThrow(NodeLoweringError);
    expect(() => subgraphDescriptor.toNodeOp({})).toThrow(/inlined before lowering/);
  });
});
