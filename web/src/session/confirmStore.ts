// A tiny modal store (#63) for blocking yes/no/cancel questions: a single
// in-flight prompt with a promise the caller awaits. Kept separate from the
// document store (pure UI ephemera, never part of an undo snapshot) and from the
// toast store (this one BLOCKS the action until answered). Used for the
// unsaved-changes prompt (save/discard/cancel) and the launch recovery offer
// (restore/discard).
import { create } from "zustand";

/**
 * The user's answer. The three positions are stable; the BUTTON LABELS are
 * per-prompt so the same store serves "Save / Discard / Cancel" and
 * "Restore / Discard" (no third button when `cancelLabel` is omitted).
 */
export type ConfirmChoice = "confirm" | "discard" | "cancel";

/** Per-prompt button labels (a missing label hides that button). */
export interface ConfirmLabels {
  /** The primary/affirmative action (e.g. "Save", "Restore"). */
  confirm: string;
  /** The discard action (e.g. "Discard", "Don't Save"). */
  discard: string;
  /** The cancel action; omit to hide it (a 2-button prompt). */
  cancel?: string;
}

/** The live prompt, or `null` when nothing is being asked. */
export interface ConfirmPrompt {
  /** The message shown (e.g. which document is about to be discarded). */
  message: string;
  /** The button labels for this prompt. */
  labels: ConfirmLabels;
  /** Resolve the awaited `ask()` promise with the user's choice. */
  resolve: (choice: ConfirmChoice) => void;
}

interface ConfirmState {
  prompt: ConfirmPrompt | null;
  /**
   * Ask a blocking question; resolves when the user answers (via `answer`). If a
   * prompt is already open it is auto-cancelled first so only one is ever live.
   */
  ask: (message: string, labels: ConfirmLabels) => Promise<ConfirmChoice>;
  /** Answer the live prompt (the modal's buttons call this), resolving the promise. */
  answer: (choice: ConfirmChoice) => void;
}

export const useConfirmStore = create<ConfirmState>((set, get) => ({
  prompt: null,

  ask: (message, labels) => {
    // Cancel any already-open prompt so we never strand two awaiters.
    const existing = get().prompt;
    if (existing) {
      existing.resolve("cancel");
    }
    return new Promise<ConfirmChoice>((resolve) => {
      set({ prompt: { message, labels, resolve } });
    });
  },

  answer: (choice) => {
    const prompt = get().prompt;
    if (!prompt) {
      return;
    }
    set({ prompt: null });
    prompt.resolve(choice);
  },
}));
