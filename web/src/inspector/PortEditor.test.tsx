import { render, screen, fireEvent } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { EditablePorts, PortSignature } from "../nodes/types";
import { PortEditor } from "./PortEditor";
import type { NodeDataEditor } from "./useNodeDataEditor";

// A stand-in editablePorts capability (the real one ships with the snippet node,
// #52) that simply stashes the edited signature under data.ports so the test can
// assert what the inspector wrote back.
const caps: EditablePorts = {
  setPorts: (_data, signature) => ({ ports: signature }) as Record<string, unknown>,
};

function fakeEditor(): NodeDataEditor & { patches: Array<Record<string, unknown>> } {
  const patches: Array<Record<string, unknown>> = [];
  return {
    patches,
    live: vi.fn(),
    flush: vi.fn(),
    commit: (patch) => {
      patches.push(patch);
    },
  };
}

const baseSignature: PortSignature = {
  inputs: [{ name: "a", type: "vec4" }],
  outputs: [{ name: "out", type: "vec4" }],
};

function lastSignature(editor: ReturnType<typeof fakeEditor>): PortSignature {
  return (editor.patches.at(-1)!.ports as PortSignature);
}

describe("PortEditor", () => {
  it("renders existing input + output ports", () => {
    const editor = fakeEditor();
    render(<PortEditor caps={caps} signature={baseSignature} data={{}} editor={editor} />);
    expect((screen.getByLabelText("Inputs port 0 name") as HTMLInputElement).value).toBe("a");
    expect((screen.getByLabelText("Outputs port 0 name") as HTMLInputElement).value).toBe("out");
  });

  it("adds an input port with a fresh non-colliding name", () => {
    const editor = fakeEditor();
    render(<PortEditor caps={caps} signature={baseSignature} data={{}} editor={editor} />);
    fireEvent.click(screen.getByRole("button", { name: /Add input/i }));
    expect(lastSignature(editor).inputs).toHaveLength(2);
  });

  it("renames a port", () => {
    const editor = fakeEditor();
    render(<PortEditor caps={caps} signature={baseSignature} data={{}} editor={editor} />);
    fireEvent.change(screen.getByLabelText("Inputs port 0 name"), { target: { value: "uv" } });
    expect(lastSignature(editor).inputs[0]!.name).toBe("uv");
  });

  it("retypes a port", () => {
    const editor = fakeEditor();
    render(<PortEditor caps={caps} signature={baseSignature} data={{}} editor={editor} />);
    fireEvent.change(screen.getByLabelText("Inputs port 0 type"), { target: { value: "vec2" } });
    expect(lastSignature(editor).inputs[0]!.type).toBe("vec2");
  });

  it("removes a port", () => {
    const editor = fakeEditor();
    render(<PortEditor caps={caps} signature={baseSignature} data={{}} editor={editor} />);
    fireEvent.click(screen.getByRole("button", { name: /Remove Inputs port 0/i }));
    expect(lastSignature(editor).inputs).toHaveLength(0);
  });
});
