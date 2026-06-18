// The node palette context menu (#49) — now driven by the node-descriptor
// REGISTRY. Every registered kind is offered, grouped by category, and inserts a
// node carrying that descriptor's `defaultData()`. Adding a node kind = adding a
// descriptor; this menu needs no change.
import type { Vec2 } from "../bindings/Vec2";
import { descriptorsByCategory, nonEmptyCategories } from "../nodes/registry";
import type { NodeCategory } from "../nodes/types";
import { useDocumentStore } from "../store/documentStore";

/** Human section headings for each category. */
const CATEGORY_LABEL: Record<NodeCategory, string> = {
  input: "Inputs / Samplers",
  coordinate: "Coordinates / UV",
  constant: "Constants",
  parameter: "Parameters",
  builtin: "Builtins",
  math: "Math",
  color: "Color",
  custom: "Custom",
  output: "Output",
};

export interface NodePaletteMenuProps {
  /** Screen-space anchor (where the user right-clicked). */
  screen: { x: number; y: number };
  /** Graph-space position the new node should be inserted at. */
  graphPosition: Vec2;
  /** Close the menu (selection made or dismissed). */
  onClose: () => void;
}

export function NodePaletteMenu({ screen, graphPosition, onClose }: NodePaletteMenuProps) {
  const addNode = useDocumentStore((s) => s.addNode);
  const setSelection = useDocumentStore((s) => s.setSelection);

  function insert(kind: string, data: Record<string, unknown>): void {
    const id = addNode(kind, graphPosition, data);
    setSelection({ nodeIds: [id], edgeIds: [] });
    onClose();
  }

  return (
    <div
      className="editor__palette"
      role="menu"
      aria-label="Insert node"
      style={{ left: screen.x, top: screen.y }}
    >
      {nonEmptyCategories().map((category) => (
        <div key={category} className="editor__palette-group">
          <div className="editor__palette-heading">{CATEGORY_LABEL[category]}</div>
          {descriptorsByCategory(category).map((descriptor) => (
            <button
              key={descriptor.kind}
              type="button"
              role="menuitem"
              className="editor__palette-item"
              title={descriptor.description}
              onClick={() => insert(descriptor.kind, descriptor.defaultData())}
            >
              {descriptor.label}
            </button>
          ))}
        </div>
      ))}
    </div>
  );
}
