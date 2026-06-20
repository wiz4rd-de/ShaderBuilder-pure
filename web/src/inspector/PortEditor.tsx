// Generic editable-port UI (#47) — drives ANY descriptor that exposes the
// `editablePorts` capability (the custom-snippet node, #52, is the only one for
// now). It renders the node's current input/output ports as rename + retype rows
// with add/remove controls, and writes the edited signature back into `node.data`
// via the descriptor's `setPorts` patch. The canvas handles + the node's declared
// signature update because they read the SAME `data` through the descriptor.
import type { PortType } from "../bindings/PortType";
import type { EditablePorts, PortSignature, PortSpec } from "../nodes/types";
import type { NodeDataEditor } from "./useNodeDataEditor";

/** The full set of port types a user may assign (drives the type dropdown). */
const PORT_TYPES: PortType[] = ["float", "vec2", "vec3", "vec4", "int", "bool", "sampler2D"];

interface PortEditorProps {
  /** The descriptor's editable-ports capability. */
  caps: EditablePorts;
  /** The node's current ports (already resolved from `descriptor.inputs/outputs`). */
  signature: PortSignature;
  /** The node's `data` (passed to `caps.setPorts`). */
  data: Record<string, unknown>;
  editor: NodeDataEditor;
}

/** Generate a fresh, non-colliding port name on a side. */
function freshName(existing: PortSpec[]): string {
  const taken = new Set(existing.map((p) => p.name));
  for (let i = existing.length; ; i++) {
    const name = `port${i}`;
    if (!taken.has(name)) {
      return name;
    }
  }
}

export function PortEditor({ caps, signature, data, editor }: PortEditorProps): React.JSX.Element {
  const allowInputs = caps.allowInputs ?? true;
  const allowOutputs = caps.allowOutputs ?? true;

  // Apply an edited signature: ask the descriptor for the data patch, commit one
  // undo entry (port edits are discrete, not coalesced like typing).
  function apply(next: PortSignature): void {
    editor.commit(caps.setPorts(data, next) as Record<string, unknown>);
  }

  function editSide(side: "inputs" | "outputs", ports: PortSpec[]): void {
    apply({ ...signature, [side]: ports });
  }

  return (
    <div className="inspector__ports-editor">
      {allowInputs ? (
        <PortSide
          title="Inputs"
          ports={signature.inputs}
          onChange={(ports) => editSide("inputs", ports)}
        />
      ) : null}
      {allowOutputs ? (
        <PortSide
          title="Outputs"
          ports={signature.outputs}
          onChange={(ports) => editSide("outputs", ports)}
        />
      ) : null}
    </div>
  );
}

interface PortSideProps {
  title: string;
  ports: PortSpec[];
  onChange: (ports: PortSpec[]) => void;
}

function PortSide({ title, ports, onChange }: PortSideProps): React.JSX.Element {
  function rename(index: number, name: string): void {
    onChange(ports.map((p, i) => (i === index ? { ...p, name } : p)));
  }
  function retype(index: number, type: PortType): void {
    onChange(ports.map((p, i) => (i === index ? { ...p, type } : p)));
  }
  function remove(index: number): void {
    onChange(ports.filter((_, i) => i !== index));
  }
  function add(): void {
    onChange([...ports, { name: freshName(ports), type: "vec4" }]);
  }

  return (
    <fieldset className="inspector__port-side">
      <legend className="inspector__port-side-title">{title}</legend>
      {ports.map((port, index) => (
        <div key={index} className="inspector__port-row">
          <input
            type="text"
            aria-label={`${title} port ${index} name`}
            className="inspector__input inspector__port-name"
            value={port.name}
            onChange={(e) => rename(index, e.target.value)}
          />
          <select
            aria-label={`${title} port ${index} type`}
            className="inspector__input inspector__port-type"
            value={port.type}
            onChange={(e) => retype(index, e.target.value as PortType)}
          >
            {PORT_TYPES.map((t) => (
              <option key={t} value={t}>
                {t}
              </option>
            ))}
          </select>
          <button
            type="button"
            className="inspector__port-remove"
            aria-label={`Remove ${title} port ${index}`}
            onClick={() => remove(index)}
          >
            ×
          </button>
        </div>
      ))}
      <button type="button" className="inspector__port-add" onClick={add}>
        + Add {title.toLowerCase().replace(/s$/, "")}
      </button>
    </fieldset>
  );
}
