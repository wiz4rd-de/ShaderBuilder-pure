// The right-rail inspector container (#47). Resolves the current single-node
// selection out of the document store and renders its NodeInspector; shows a
// neutral placeholder when nothing, multiple nodes, or only edges are selected,
// or when the canvas is in the pipeline view (no graph node to edit).
//
// Layout note: #47 lands a simple right-rail so the inspector has a home. #48
// owns the dockable/tabbed right-region layout and will slot this (and the other
// panels) into it; until then this is a standalone column in App's right region.
import { hasDescriptor } from "../nodes/registry";
import { useDocumentStore } from "../store/documentStore";
import { NodeInspector } from "./NodeInspector";

export function InspectorPanel(): React.JSX.Element {
  const level = useDocumentStore((s) => s.level);
  const selection = useDocumentStore((s) => s.selection);
  const graph = useDocumentStore((s) => s.activeGraph());

  const nodeIds = selection.nodeIds;
  const single = level === "pass" && nodeIds.length === 1 ? nodeIds[0]! : null;
  const node = single ? graph.nodes.find((n) => n.id === single) : undefined;

  return (
    <section className="inspector" aria-label="Inspector">
      <div className="inspector__header">Inspector</div>
      <div className="inspector__body">
        <InspectorBody
          level={level}
          nodeCount={nodeIds.length}
          node={node ?? null}
        />
      </div>
    </section>
  );
}

function InspectorBody({
  level,
  nodeCount,
  node,
}: {
  level: "pipeline" | "pass";
  nodeCount: number;
  node: import("../bindings/Node").Node | null;
}): React.JSX.Element {
  if (level === "pipeline") {
    return <Placeholder text="Drill into a pass to edit its nodes." />;
  }
  if (nodeCount === 0) {
    return <Placeholder text="Select a node to edit its properties." />;
  }
  if (nodeCount > 1) {
    return <Placeholder text={`${nodeCount} nodes selected.`} />;
  }
  if (!node) {
    return <Placeholder text="Select a node to edit its properties." />;
  }
  if (!hasDescriptor(node.kind)) {
    return <Placeholder text={`Unknown node kind "${node.kind}".`} />;
  }
  // `key` resets the per-field local state when the selected node changes.
  return <NodeInspector key={node.id} node={node} />;
}

function Placeholder({ text }: { text: string }): React.JSX.Element {
  return <div className="inspector__placeholder">{text}</div>;
}
