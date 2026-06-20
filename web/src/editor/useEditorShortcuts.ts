// Global keyboard shortcuts for the editor (#45). Wired once by EditorCanvas.
// Bindings (Cmd on mac, Ctrl elsewhere):
//   undo            Ctrl/Cmd+Z
//   redo            Ctrl/Cmd+Shift+Z  (and Ctrl/Cmd+Y)
//   copy            Ctrl/Cmd+C
//   paste           Ctrl/Cmd+V
//   duplicate       Ctrl/Cmd+D
//   delete          Delete / Backspace
//   new             Ctrl/Cmd+N        (#63)
//   open            Ctrl/Cmd+O        (#63)
//   save            Ctrl/Cmd+S        (#63)
//   save as         Ctrl/Cmd+Shift+S  (#63)
// The EDIT shortcuts are ignored while focus is in a text input/textarea/
// contenteditable so typing into an inspector field never mutates the graph. The
// FILE shortcuts (New/Open/Save/Save-As) are handled first and fire regardless of
// focus, so Ctrl+S works while typing in the code editor.
import { useEffect } from "react";

import { useDocumentStore } from "../store/documentStore";
import { newProject, openProject, save, saveAs } from "../session/projectActions";

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
      const mod = event.ctrlKey || event.metaKey;

      // File-menu shortcuts (#63) are handled FIRST and regardless of focus, so
      // Ctrl+S saves even while typing in the code editor or an inspector field.
      if (mod && (event.key === "s" || event.key === "S")) {
        event.preventDefault();
        if (event.shiftKey) {
          void saveAs();
        } else {
          void save();
        }
        return;
      }
      if (mod && !event.shiftKey && (event.key === "n" || event.key === "N")) {
        event.preventDefault();
        void newProject();
        return;
      }
      if (mod && !event.shiftKey && (event.key === "o" || event.key === "O")) {
        event.preventDefault();
        void openProject();
        return;
      }

      if (isEditableTarget(event.target)) {
        return;
      }
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
