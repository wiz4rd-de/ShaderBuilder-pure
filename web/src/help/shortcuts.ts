// The keyboard-shortcut reference shown in the in-app Help dialog (#66). This is
// the human-facing mirror of `editor/useEditorShortcuts` (the binding authority)
// plus the React-Flow pan/zoom gestures — keep it in sync when bindings change.

export interface Shortcut {
  /** What the binding does. */
  label: string;
  /** One or more key combos that trigger it (rendered as <kbd>). */
  keys: string[];
}

export interface ShortcutGroup {
  title: string;
  shortcuts: Shortcut[];
}

export const SHORTCUT_GROUPS: ShortcutGroup[] = [
  {
    title: "File",
    shortcuts: [
      { label: "New project", keys: ["Ctrl+N"] },
      { label: "Open project", keys: ["Ctrl+O"] },
      { label: "Save", keys: ["Ctrl+S"] },
      { label: "Save As", keys: ["Ctrl+Shift+S"] },
    ],
  },
  {
    title: "Edit",
    shortcuts: [
      { label: "Undo", keys: ["Ctrl+Z"] },
      { label: "Redo", keys: ["Ctrl+Shift+Z", "Ctrl+Y"] },
      { label: "Copy", keys: ["Ctrl+C"] },
      { label: "Paste", keys: ["Ctrl+V"] },
      { label: "Duplicate", keys: ["Ctrl+D"] },
      { label: "Delete selection", keys: ["Delete", "Backspace"] },
    ],
  },
  {
    title: "Canvas",
    shortcuts: [
      { label: "Pan", keys: ["Space + drag", "Middle-mouse drag"] },
      { label: "Zoom", keys: ["Scroll", "Ctrl+Scroll"] },
      { label: "Add to selection", keys: ["Shift + click"] },
      { label: "Box-select", keys: ["Shift + drag"] },
    ],
  },
];
