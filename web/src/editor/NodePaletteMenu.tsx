// A minimal node palette shown as a context menu on the canvas (#45). It offers
// the single generic placeholder node so the canvas is usable BEFORE the real
// taxonomy (#49) lands; #49 replaces this list with the node-descriptor registry.
import type { Vec2 } from "../bindings/Vec2";
import { useDocumentStore } from "../store/documentStore";
import { PLACEHOLDER_KIND } from "../store/factories";

/** One palette entry: the node kind to insert and how to label it. */
interface PaletteEntry {
  kind: string;
  label: string;
}

// Until #49's registry exists, the palette is a single generic node. Adding to
// this array is the temporary way to surface more node kinds.
const PALETTE: PaletteEntry[] = [{ kind: PLACEHOLDER_KIND, label: "Placeholder node" }];

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

  function insert(kind: string): void {
    const id = addNode(kind, graphPosition);
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
      {PALETTE.map((entry) => (
        <button
          key={entry.kind}
          type="button"
          role="menuitem"
          className="editor__palette-item"
          onClick={() => insert(entry.kind)}
        >
          {entry.label}
        </button>
      ))}
    </div>
  );
}
