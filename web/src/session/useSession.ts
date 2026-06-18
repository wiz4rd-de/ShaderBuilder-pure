// The session lifecycle hook (#63), wired once by App:
//
//  * DIRTY MIRROR — subscribes to the document store's `dirty` flag and pushes it
//    to the backend (`set_dirty`) so the Rust window-close handler can see it (it
//    cannot read JS state). Fires only on a real dirty TRANSITION.
//  * AUTOSAVE — debounced mirror of the working document to the recovery file, so
//    a crash/kill leaves recoverable work. Debounced (and only when dirty) so it
//    does not fight the live compile loop or thrash the disk.
//  * CLOSE GUARD — listens for the backend's `close-requested` event (emitted when
//    the user tries to close with unsaved edits) and runs the save/discard/cancel
//    prompt; on proceed it clears dirty and re-issues the close so the now-clean
//    handler lets it through.
//  * RECOVERY OFFER — on launch, asks the backend for a recovery newer than the
//    last save and, if any, prompts the user to restore it.
//
// All Tauri seams (listen, the window, the recovery check) are injectable so the
// hook unit-tests with no Tauri runtime.
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useEffect } from "react";

import type { Recovery } from "../bindings/Recovery";
import { useOnboardingStore } from "../onboarding/onboardingStore";
import { useConfirmStore } from "./confirmStore";
import { useDocumentStore } from "../store/documentStore";
import { useToastStore } from "../feedback/toastStore";
import {
  autosaveRecovery as defaultAutosave,
  checkRecovery as defaultCheckRecovery,
  clearRecovery as defaultClearRecovery,
  setBackendDirty as defaultSetBackendDirty,
} from "./api";
import { guardUnsaved, restoreRecovery } from "./projectActions";

/** How long after the last edit before the working doc is autosaved (#63). */
export const AUTOSAVE_DEBOUNCE_MS = 2000;

/** The injectable Tauri/IO seams (defaults are the real ones). */
export interface SessionHookDeps {
  setBackendDirty: (dirty: boolean) => Promise<void>;
  autosaveRecovery: (
    project: import("../bindings/Project").Project,
    projectPath: string | null,
  ) => Promise<void>;
  checkRecovery: () => Promise<Recovery | null>;
  /** Subscribe to the backend `close-requested` event; returns an unlisten fn. */
  onCloseRequested: (handler: () => void) => Promise<UnlistenFn>;
  /** Actually close the window once the guard cleared it. */
  closeWindow: () => Promise<void>;
}

const defaultDeps: SessionHookDeps = {
  setBackendDirty: defaultSetBackendDirty,
  autosaveRecovery: defaultAutosave,
  checkRecovery: defaultCheckRecovery,
  onCloseRequested: (handler) => listen("close-requested", () => handler()),
  closeWindow: () => getCurrentWindow().destroy(),
};

export function useSession(deps: SessionHookDeps = defaultDeps): void {
  // DIRTY MIRROR: push every dirty transition to the backend.
  useEffect(() => {
    // Sync the initial value, then on each change.
    void deps.setBackendDirty(useDocumentStore.getState().dirty).catch(() => undefined);
    let last = useDocumentStore.getState().dirty;
    const unsub = useDocumentStore.subscribe((s) => {
      if (s.dirty !== last) {
        last = s.dirty;
        void deps.setBackendDirty(last).catch(() => undefined);
      }
    });
    return unsub;
  }, [deps]);

  // AUTOSAVE: debounce a recovery write after edits, only while dirty.
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;
    const schedule = (): void => {
      if (timer) {
        clearTimeout(timer);
      }
      timer = setTimeout(() => {
        const { project, currentProjectPath, dirty } = useDocumentStore.getState();
        if (!dirty) {
          return;
        }
        void deps.autosaveRecovery(project, currentProjectPath).catch(() => undefined);
      }, AUTOSAVE_DEBOUNCE_MS);
    };
    let lastProject = useDocumentStore.getState().project;
    const unsub = useDocumentStore.subscribe((s) => {
      // Re-arm on any document change (the doc reference changes on every edit).
      if (s.project !== lastProject && s.dirty) {
        lastProject = s.project;
        schedule();
      } else {
        lastProject = s.project;
      }
    });
    return () => {
      if (timer) {
        clearTimeout(timer);
      }
      unsub();
    };
  }, [deps]);

  // CLOSE GUARD: the backend vetoed a close because we are dirty; ask the user.
  useEffect(() => {
    let unlisten: UnlistenFn | null = null;
    let disposed = false;
    void deps
      .onCloseRequested(() => {
        void (async () => {
          const proceed = await guardUnsaved();
          if (!proceed) {
            return; // user cancelled / save failed — stay open
          }
          // Clear dirty so the backend handler lets the close through, then close.
          useDocumentStore.getState().markSaved();
          await deps.closeWindow().catch(() => undefined);
        })();
      })
      .then((off) => {
        if (disposed) {
          off();
        } else {
          unlisten = off;
        }
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [deps]);

  // RECOVERY OFFER: on launch, offer to restore a newer autosave.
  useEffect(() => {
    let cancelled = false;
    void deps
      .checkRecovery()
      .then(async (recovery) => {
        if (cancelled || !recovery) {
          return;
        }
        const choice = await useConfirmStore
          .getState()
          .ask(`Recover unsaved work from "${recovery.meta.name}"?`, {
            confirm: "Restore",
            discard: "Discard",
          });
        if (cancelled) {
          return;
        }
        if (choice === "confirm") {
          await restoreRecovery(recovery.project, recovery.meta.projectPath ?? null);
          // A restored doc skips the start screen (#66): the user has work to edit.
          useOnboardingStore.getState().markStarted();
          useToastStore.getState().push("info", "Recovered unsaved work.");
        } else {
          // Discard / cancel: drop the recovery so we do not re-offer it.
          await defaultClearRecovery().catch(() => undefined);
        }
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [deps]);
}
