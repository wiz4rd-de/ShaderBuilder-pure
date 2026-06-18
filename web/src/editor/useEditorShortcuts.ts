// Global keyboard shortcuts for the editor (#45). Wired once by EditorCanvas.
// Bindings (Cmd on mac, Ctrl elsewhere):
//   undo            Ctrl/Cmd+Z
//   redo            Ctrl/Cmd+Shift+Z  (and Ctrl/Cmd+Y)
//   copy            Ctrl/Cmd+C
//   paste           Ctrl/Cmd+V
//   duplicate       Ctrl/Cmd+D
//   delete          Delete / Backspace
// Shortcuts are ignored while focus is in a text input/textarea/contenteditable
// so typing into a future inspector field never mutates the graph.
import { useEffect } from "react";

import { useDocumentStore } from "../store/documentStore";

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  const tag = target.tagName;
  return (
    tag === "INPUT" ||
    tag === "TEXTAREA" ||
    tag === "SELECT" ||
    target.isContentEditable
  );
}

export function useEditorShortcuts(): void {
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent): void {
      if (isEditableTarget(event.target)) {
        return;
      }
      const mod = event.ctrlKey || event.metaKey;
      const store = useDocumentStore.getState();

      if (mod && (event.key === "z" || event.key === "Z")) {
        event.preventDefault();
        if (event.shiftKey) {
          store.redo();
        } else {
          store.undo();
        }
        return;
      }
      if (mod && (event.key === "y" || event.key === "Y")) {
        event.preventDefault();
        store.redo();
        return;
      }
      if (mod && (event.key === "c" || event.key === "C")) {
        store.copy();
        return;
      }
      if (mod && (event.key === "v" || event.key === "V")) {
        store.paste();
        return;
      }
      if (mod && (event.key === "d" || event.key === "D")) {
        event.preventDefault();
        store.duplicate();
        return;
      }
      if (event.key === "Delete" || event.key === "Backspace") {
        event.preventDefault();
        store.removeSelection();
      }
    }

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);
}
