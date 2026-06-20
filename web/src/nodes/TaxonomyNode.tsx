// The custom React Flow node component (#49) — ONE component renders EVERY node
// kind by reading its descriptor from the registry. The canvas registers it under
// each kind (see nodeTypes.ts), so the node's title + typed port handles always
// reflect the descriptor's `inputs`/`outputs` (which may depend on `node.data`,
// e.g. a Const's output type). Adding a node kind needs no new component.
import { Handle, Position, type NodeProps } from "@xyflow/react";

import { useDocumentStore } from "../store/documentStore";
import { getDescriptor } from "./registry";
import type { NodeData, PortSpec } from "./types";

/** The RF node data shape: the document node's free-form data (plus a label). */
export type TaxonomyNodeData = NodeData & { label?: string };

/** A short per-type accent class so categories are visually distinguishable. */
function categoryClass(kind: string): string {
  const cat = getDescriptor(kind)?.category ?? "unknown";
  return `taxonomy-node--${cat}`;
}

/** One input/output handle row, labelled, with the typed connection point. */
function PortRow({
  port,
  side,
}: {
  port: PortSpec;
  side: "target" | "source";
}): React.JSX.Element {
  const position = side === "target" ? Position.Left : Position.Right;
  return (
    <div className={`taxonomy-node__port taxonomy-node__port--${side}`}>
      <Handle
        type={side}
        position={position}
        id={port.name}
        className={`taxonomy-node__handle taxonomy-node__handle--${port.type}`}
      />
      <span className="taxonomy-node__port-label">{port.label ?? port.name}</span>
    </div>
  );
}

/**
 * Render a taxonomy node. `props.type` is the document node's `kind`; `props.data`
 * is its `data`. Falls back to a minimal "unknown kind" card so a stale/unknown
 * node still renders (rather than crashing the canvas).
 */
export function TaxonomyNode(props: NodeProps): React.JSX.Element {
  const kind = props.type;
  const descriptor = getDescriptor(kind);
  const data = props.data as TaxonomyNodeData;

  // Inline diagnostics (#54): the live compile loop keys each Diagnostic by the
  // offending IrNode id (== this node's id). Surface the worst severity as a badge
  // + tooltip so an invalid node is flagged on the canvas, not just in the inspector.
  const diagnostics = useDocumentStore((s) => s.diagnosticsByNode[props.id]);
  const severity =
    diagnostics && diagnostics.length > 0
      ? diagnostics.some((d) => d.severity === "error")
        ? "error"
        : "warning"
      : null;

  if (!descriptor) {
    return (
      <div className="taxonomy-node taxonomy-node--unknown">
        <div className="taxonomy-node__title">{kind}</div>
        <div className="taxonomy-node__subtitle">unknown node</div>
      </div>
    );
  }

  const inputs = descriptor.inputs(data);
  const outputs = descriptor.outputs(data);
  const title = typeof data.label === "string" && data.label.length > 0 ? data.label : descriptor.label;

  return (
    <div
      className={`taxonomy-node ${categoryClass(kind)}${
        severity ? ` taxonomy-node--${severity}` : ""
      }`}
      data-kind={kind}
      data-diagnostic={severity ?? undefined}
    >
      <div className="taxonomy-node__title">
        {title}
        {severity ? (
          <span
            className={`taxonomy-node__badge taxonomy-node__badge--${severity}`}
            title={diagnostics!.map((d) => `${d.code}: ${d.message}`).join("\n")}
            aria-label={`${severity}: ${diagnostics!.length} ${
              diagnostics!.length === 1 ? "diagnostic" : "diagnostics"
            }`}
          >
            {severity === "error" ? "!" : "?"}
          </span>
        ) : null}
      </div>
      <div className="taxonomy-node__ports">
        <div className="taxonomy-node__col taxonomy-node__col--in">
          {inputs.map((p) => (
            <PortRow key={`in-${p.name}`} port={p} side="target" />
          ))}
        </div>
        <div className="taxonomy-node__col taxonomy-node__col--out">
          {outputs.map((p) => (
            <PortRow key={`out-${p.name}`} port={p} side="source" />
          ))}
        </div>
      </div>
    </div>
  );
}
