// The single-node inspector form (#47). Renders ENTIRELY from the selected node's
// registry descriptor: its label, its typed in/out ports, its field schema, and —
// for editable-port nodes (custom snippet, #52) — the generic port editor. Edits
// flow back into the document store (and thus the next compile_graph) via the
// NodeDataEditor. Any diagnostics keyed by this node id (populated by #54) render
// read-only at the bottom.
import type { Node } from "../bindings/Node";
import { requireDescriptor } from "../nodes/registry";
import type { PortSpec } from "../nodes/types";
import { useDocumentStore } from "../store/documentStore";
import { InspectorFieldRow } from "./InspectorFields";
import { PortEditor } from "./PortEditor";
import { useNodeDataEditor } from "./useNodeDataEditor";

interface NodeInspectorProps {
  node: Node;
}

export function NodeInspector({ node }: NodeInspectorProps): React.JSX.Element {
  const editor = useNodeDataEditor(node.id);
  const diagnostics = useDocumentStore((s) => s.diagnosticsByNode[node.id]);
  const descriptor = requireDescriptor(node.kind);
  const { data } = node;

  const inputs = descriptor.inputs(data);
  const outputs = descriptor.outputs(data);
  const fields = descriptor.inspector(data);
  const editablePorts = descriptor.editablePorts;

  return (
    <div className="inspector__node">
      <header className="inspector__node-head">
        <div className="inspector__node-title">{descriptor.label}</div>
        <div className="inspector__node-id" title="Node id">
          {node.id}
        </div>
      </header>

      {fields.length > 0 ? (
        <section className="inspector__section">
          {fields.map((field) => (
            <InspectorFieldRow key={field.key} field={field} data={data} editor={editor} />
          ))}
        </section>
      ) : null}

      {editablePorts ? (
        <section className="inspector__section">
          <PortEditor
            caps={editablePorts}
            signature={{ inputs, outputs }}
            data={data}
            editor={editor}
          />
        </section>
      ) : (
        <section className="inspector__section inspector__ports">
          <PortList title="Inputs" ports={inputs} />
          <PortList title="Outputs" ports={outputs} />
        </section>
      )}

      {diagnostics && diagnostics.length > 0 ? (
        <section className="inspector__section inspector__diagnostics">
          {diagnostics.map((d, i) => (
            <div
              key={i}
              className={`inspector__diagnostic inspector__diagnostic--${d.severity}`}
            >
              <span className="inspector__diagnostic-code">{d.code}</span>
              <span className="inspector__diagnostic-message">{d.message}</span>
            </div>
          ))}
        </section>
      ) : null}
    </div>
  );
}

/** Read-only listing of a side's ports with their declared types. */
function PortList({ title, ports }: { title: string; ports: PortSpec[] }): React.JSX.Element {
  if (ports.length === 0) {
    return (
      <div className="inspector__port-list">
        <div className="inspector__port-list-title">{title}</div>
        <div className="inspector__port-list-empty">none</div>
      </div>
    );
  }
  return (
    <div className="inspector__port-list">
      <div className="inspector__port-list-title">{title}</div>
      {ports.map((p) => (
        <div key={p.name} className="inspector__port-list-row">
          <span className="inspector__port-list-name">{p.label ?? p.name}</span>
          <span className={`inspector__port-list-type inspector__port-list-type--${p.type}`}>
            {p.type}
          </span>
        </div>
      ))}
    </div>
  );
}
