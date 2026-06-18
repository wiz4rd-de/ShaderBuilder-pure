// A tiny non-blocking toast/banner store (#62) — transient, dismissable messages
// surfaced for engine/IO/render errors so a failure is VISIBLE without a modal or
// a crash. Kept independent of the document store: toasts are pure UI ephemera,
// never part of an undo snapshot or the project.
//
// Toasts auto-dismiss after a timeout (errors linger longer than info) and can be
// dismissed manually. De-duped by (severity, message) within the live set so a
// per-frame repeat of the same render error does not stack into a wall of toasts.
import { create } from "zustand";

/** A toast's severity — drives its styling and how long it lingers. */
export type ToastSeverity = "error" | "warning" | "info";

/** One live toast. */
export interface Toast {
  id: string;
  severity: ToastSeverity;
  message: string;
}

/** How long (ms) a toast of each severity lingers before auto-dismiss. */
const LINGER_MS: Record<ToastSeverity, number> = {
  error: 8000,
  warning: 6000,
  info: 4000,
};

interface ToastState {
  toasts: Toast[];
  /**
   * Show a toast. De-dupes against a live toast with the same severity+message
   * (a repeated render error does not stack). Returns the toast id (or the
   * existing one's id when de-duped). Auto-dismisses after the severity timeout.
   */
  push: (severity: ToastSeverity, message: string) => string;
  /** Dismiss a toast by id (manual close or the auto-dismiss timer). */
  dismiss: (id: string) => void;
  /** Clear every toast (e.g. on project load). */
  clear: () => void;
}

let nextToastId = 0;

export const useToastStore = create<ToastState>((set, get) => ({
  toasts: [],

  push: (severity, message) => {
    const existing = get().toasts.find(
      (t) => t.severity === severity && t.message === message,
    );
    if (existing) {
      return existing.id;
    }
    const id = `toast-${nextToastId++}`;
    set((s) => ({ toasts: [...s.toasts, { id, severity, message }] }));
    // Auto-dismiss; the timer is fire-and-forget (manual dismiss is idempotent).
    setTimeout(() => get().dismiss(id), LINGER_MS[severity]);
    return id;
  },

  dismiss: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),

  clear: () => set({ toasts: [] }),
}));
