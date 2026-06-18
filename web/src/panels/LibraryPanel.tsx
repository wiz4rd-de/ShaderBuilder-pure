// The personal-library browser panel (#59) — the frontend workflow over the
// Phase-6 library store (#56 schema, #58 IO commands, #57 subgraph model):
//
//   * LIST persisted items (cross-project: they live on disk, #58) with their
//     name / kind (single node vs subgraph) / tags / description.
//   * SAVE the current selection: one selected node -> a {kind:"node"} payload;
//     a multi-selection -> a {kind:"subgraph"} payload built with #57's collapse
//     boundary logic (without mutating the live graph). Prompts for a name.
//   * INSERT (instantiate) the selected item into the active per-pass graph with
//     FRESH ids (the #56 algorithm, mirrored in instantiate.ts) — a subgraph item
//     drops in as a collapsed, drill-in-editable node; a single node drops in as
//     that node. Goes THROUGH the store so undo history + the debounced compile
//     loop (#54) fire automatically.
//   * DELETE an item (behind a confirm) and refresh.
import { useCallback, useEffect, useState } from "react";

import type { LibraryItem } from "../bindings/LibraryItem";
import type { LibraryPayload } from "../bindings/LibraryPayload";
import type { Subgraph } from "../bindings/Subgraph";
import type { Vec2 } from "../bindings/Vec2";
import { collapseSelection as collapse } from "../store/collapse";
import { useDocumentStore } from "../store/documentStore";
import { nextId } from "../store/ids";
import { readSubgraph, SUBGRAPH_KIND } from "../nodes/subgraph";
import {
  deleteLibraryNode,
  listLibraryNode,
  saveLibraryNode,
} from "../library/api";
import { instantiateLibraryItem } from "../library/instantiate";

/** A label for a payload's "single node vs subgraph" kind (issue wording). */
function payloadKindLabel(payload: LibraryPayload): string {
  return payload.kind === "subgraph" ? "subgraph" : "node";
}

/**
 * Build the payload for the CURRENT selection in the active graph, or `null`
 * when nothing usable is selected. A single selected node -> a node payload; a
 * multi-selection -> a subgraph payload built from #57's collapse boundary logic
 * (run on a throwaway, never applied to the live graph).
 */
function payloadFromSelection(name: string): LibraryPayload | null {
  const state = useDocumentStore.getState();
  if (state.level !== "pass") {
    return null;
  }
  const graph = state.activeGraph();
  const nodeIds = state.selection.nodeIds;
  if (nodeIds.length === 0) {
    return null;
  }
  if (nodeIds.length === 1) {
    const node = graph.nodes.find((n) => n.id === nodeIds[0]);
    if (!node) {
      return null;
    }
    return { kind: "node", node };
  }
  // Multi-selection: collapse into a subgraph body (pure; not applied).
  const result = collapse(graph, nodeIds, name, nextId);
  if (!result) {
    return null;
  }
  const wrapper = result.graph.nodes.find((n) => n.kind === SUBGRAPH_KIND);
  if (!wrapper) {
    return null;
  }
  const subgraph: Subgraph = readSubgraph(wrapper);
  return { kind: "subgraph", subgraph };
}

export function LibraryPanel(): React.JSX.Element {
  const [items, setItems] = useState<LibraryItem[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const level = useDocumentStore((s) => s.level);
  const selectionCount = useDocumentStore((s) => s.selection.nodeIds.length);
  const insertLibraryPayload = useDocumentStore((s) => s.insertLibraryPayload);
  const currentViewport = useDocumentStore((s) => s.currentViewport);

  const refresh = useCallback(async () => {
    setError(null);
    try {
      setItems(await listLibraryNode());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  // List on mount (panel open). Refresh after save/delete keeps it current.
  useEffect(() => {
    void refresh();
  }, [refresh]);

  const canSave = level === "pass" && selectionCount > 0;

  const onSave = useCallback(async () => {
    const name = window.prompt("Library item name?");
    if (name === null) {
      return; // cancelled
    }
    const trimmed = name.trim();
    if (trimmed.length === 0) {
      return;
    }
    const payload = payloadFromSelection(trimmed);
    if (!payload) {
      setError("Select a node (or several) in a pass graph to save.");
      return;
    }
    const description = window.prompt("Description (optional)?") ?? "";
    const tagsRaw = window.prompt("Tags (comma-separated, optional)?") ?? "";
    const tags = tagsRaw
      .split(",")
      .map((t) => t.trim())
      .filter((t) => t.length > 0);
    const item: LibraryItem = {
      // The persisted library item id is an on-disk primary key (save_item names
      // <id>.json), so it MUST be durable-unique ACROSS sessions. `nextId` is a
      // per-process counter that resets to 0 every launch (fine for in-document
      // node/edge ids, NOT for a persistent key) — a second session would reuse
      // `lib-1` and silently overwrite a prior item. `crypto.randomUUID` is
      // durable + collision-free (available in jsdom/Node + the Tauri webview).
      id: crypto.randomUUID(),
      name: trimmed,
      description: description.trim().length > 0 ? description.trim() : null,
      tags,
      payload,
    };
    setBusy(true);
    setError(null);
    try {
      await saveLibraryNode(item);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, [refresh]);

  const onInsert = useCallback(
    (item: LibraryItem) => {
      // Fresh ids for everything inside (the #56 contract); the store mints the
      // wrapping node id and drops it in via history + auto-compile.
      const payload = instantiateLibraryItem(item, nextId);
      const vp = currentViewport();
      // Place at a sensible canvas position: the remembered viewport's origin
      // (with a small cascade offset), else a default near the top-left.
      const base: Vec2 = vp ? { x: -vp.x / vp.zoom, y: -vp.y / vp.zoom } : { x: 64, y: 64 };
      const position: Vec2 = { x: base.x + 48, y: base.y + 48 };
      insertLibraryPayload(payload, position);
    },
    [currentViewport, insertLibraryPayload],
  );

  const onDelete = useCallback(
    async (item: LibraryItem) => {
      if (!window.confirm(`Delete library item "${item.name}"?`)) {
        return;
      }
      setBusy(true);
      setError(null);
      try {
        await deleteLibraryNode(item.id);
        await refresh();
      } catch (e) {
        setError(String(e));
      } finally {
        setBusy(false);
      }
    },
    [refresh],
  );

  return (
    <section className="library" aria-label="Library">
      <div className="library__toolbar">
        <button
          type="button"
          onClick={() => void onSave()}
          disabled={!canSave || busy}
          title={
            canSave
              ? "Save the current selection to the library"
              : "Select a node (or several) in a pass graph to save"
          }
        >
          Save selection
        </button>
        <button type="button" onClick={() => void refresh()} disabled={busy}>
          Refresh
        </button>
      </div>

      {error ? (
        <div className="library__error" role="alert">
          {error}
        </div>
      ) : null}

      {items.length === 0 ? (
        <div className="library__empty">No library items yet.</div>
      ) : (
        <ul className="library__list" aria-label="Library items">
          {items.map((item) => (
            <li key={item.id} className="library__item">
              <div className="library__item-head">
                <span className="library__item-name">{item.name}</span>
                <span className="library__item-kind">{payloadKindLabel(item.payload)}</span>
              </div>
              {item.description ? (
                <div className="library__item-desc">{item.description}</div>
              ) : null}
              {item.tags.length > 0 ? (
                <div className="library__item-tags">
                  {item.tags.map((tag) => (
                    <span key={tag} className="library__item-tag">
                      {tag}
                    </span>
                  ))}
                </div>
              ) : null}
              <div className="library__item-actions">
                <button
                  type="button"
                  onClick={() => onInsert(item)}
                  disabled={level !== "pass" || busy}
                  title={
                    level === "pass"
                      ? "Insert into the active pass graph"
                      : "Drill into a pass to insert"
                  }
                >
                  Insert
                </button>
                <button
                  type="button"
                  onClick={() => void onDelete(item)}
                  disabled={busy}
                >
                  Delete
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
