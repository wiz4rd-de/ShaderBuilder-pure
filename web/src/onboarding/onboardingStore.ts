// First-run / start-screen visibility (#66). The document store always holds a
// default project (a fresh untitled doc), so "no project loaded yet" is not a
// document-store fact — it is a SESSION fact: has the user chosen a starting
// point (New / Open / Import / Open example), or recovered work, this launch?
//
// This tiny store tracks exactly that. `started` is false on launch → App shows
// the StartScreen overlay; the start-screen actions (and the recovery restore)
// flip it true → the editor takes over. It is intentionally separate from the
// document store so it is never part of an undo snapshot and a `reset()` of the
// document (File ▸ New from inside the editor) does NOT bounce the user back to
// the start screen.
import { create } from "zustand";

interface OnboardingState {
  /** Whether the user has chosen a starting point this launch. */
  started: boolean;
  /** Mark the session as started — hides the start screen. Idempotent. */
  markStarted: () => void;
  /** Reset to the start screen (used by tests; no in-app action returns here). */
  reset: () => void;
}

export const useOnboardingStore = create<OnboardingState>((set) => ({
  started: false,
  markStarted: () => set({ started: true }),
  reset: () => set({ started: false }),
}));
