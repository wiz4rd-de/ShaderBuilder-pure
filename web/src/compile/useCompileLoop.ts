// The debounced live compile loop hook (#54) — wires the document store to
// `compile_graph` + the engine preview.
//
// On every document edit it (after a DEBOUNCE) snapshots the project, lowers each
// graph pass to IR (graphToIr, #49), calls `compile_graph` per pass (#42), maps the
// diagnostics to node ids (the store's `diagnosticsByNode`, which the inspector #47
// and node badges read), and pushes the generated slang chain to the engine so the
// preview reflects the new modules.
//
// COALESCING: rapid edits collapse to ONE in-flight compile. Each dispatch is tagged
// with a monotonically-increasing edit SEQUENCE number; when a compile resolves, a
// stale result (a newer edit was dispatched meanwhile) is DROPPED — only the latest
// wins. Combined with the debounce, a typing/dragging burst is one compile, not a
// per-keystroke storm.
//
// AUTHORITY NOTE: the authoritative type-checking + connection validity live in `ir`
// (Phase 4) and are surfaced here as diagnostics. The STRICT in-editor type-checked
// connection rule (rejecting an invalid edge at drag time) is DEFERRED to Phase 7 —
// see the `onConnect` handler in editor/EditorCanvas.tsx where the connection logic
// lives.
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef } from "react";

import type { CompileGraphResult } from "../bindings/CompileGraphResult";
import type { Project } from "../bindings/Project";
import { useDocumentStore } from "../store/documentStore";
import {
  compileProject,
  type InvokeCompile,
  type ProjectCompileResult,
} from "./compileLoop";
import { dispatchPreview, type Invoke } from "./previewDispatch";

/** Debounce window: coalesce an edit burst into one compile (GOTCHA: 150–300ms). */
export const COMPILE_DEBOUNCE_MS = 200;

/** The real `compile_graph` IPC caller. */
const invokeCompile: InvokeCompile = (args) =>
  invoke<CompileGraphResult>("compile_graph", args);

/** Options for the loop (tests inject fakes + a 0ms debounce). */
export interface CompileLoopOptions {
  debounceMs?: number;
  invokeCompile?: InvokeCompile;
  invokeIpc?: Invoke;
  /** Called with each completed (non-stale) compile result (tests assert on it). */
  onResult?: (result: ProjectCompileResult) => void;
}

/**
 * Run the debounced compile loop for the lifetime of the host component. Subscribes
 * to the document store's `project`; on each change it (after the debounce) compiles
 * every pass, writes the node-keyed diagnostics into the store, dispatches the
 * generated chain to the engine preview, and reports status via `onStatus`.
 *
 * The loop owns no React state — it writes diagnostics straight into the store and
 * reports status through the callback, so it can live anywhere in the tree.
 */
export function useCompileLoop(options: CompileLoopOptions = {}): void {
  const {
    debounceMs = COMPILE_DEBOUNCE_MS,
    invokeCompile: compile = invokeCompile,
    invokeIpc = invoke,
    onResult,
  } = options;

  const setCompileStatus = useDocumentStore((s) => s.setCompileStatus);
  const setCompiling = useDocumentStore((s) => s.setCompiling);

  // Keep the latest options/callbacks in a ref so the subscription effect runs
  // once (re-subscribing on every render would thrash the debounce).
  const cbRef = useRef({ compile, invokeIpc, onResult, setCompileStatus, setCompiling });
  cbRef.current = { compile, invokeIpc, onResult, setCompileStatus, setCompiling };

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;
    // The edit sequence: every scheduled compile captures the seq it was queued
    // for; a resolved compile whose seq is no longer the latest is DROPPED.
    let editSeq = 0;
    let latestDispatched = 0;
    let disposed = false;

    const runCompile = (project: Project, seq: number): void => {
      cbRef.current.setCompiling(true);
      void compileAndDispatch(project, seq);
    };

    const compileAndDispatch = async (project: Project, seq: number): Promise<void> => {
      latestDispatched = seq;
      let result: ProjectCompileResult;
      try {
        result = await compileProject(project, cbRef.current.compile);
      } catch (err) {
        // A transport failure shouldn't tear down the loop; report + bail this round.
        console.error("compile_graph failed", err);
        if (!disposed && seq === latestDispatched) {
          cbRef.current.setCompiling(false);
        }
        return;
      }
      // Drop a stale result: a newer edit was dispatched while we were compiling.
      if (disposed || seq !== latestDispatched) {
        return;
      }
      // Apply diagnostics + problems + validity + per-pass generated source in one
      // update (clears stale ones). The per-pass source feeds the read-only
      // generated-code viewer (#55) — the SAME source the preview ran below.
      cbRef.current.setCompileStatus({
        diagnosticsByNode: result.diagnosticsByNode,
        problems: result.problems,
        valid: result.valid,
        sourcesByPassId: Object.fromEntries(
          result.passes.map((p) => [p.passId, p.source]),
        ),
      });
      cbRef.current.onResult?.(result);
      // Push the renderable chain to the engine (no-op when not globally valid).
      try {
        await dispatchPreview(result, cbRef.current.invokeIpc);
      } catch (err) {
        console.error("preview dispatch failed", err);
      }
    };

    const schedule = (project: Project): void => {
      editSeq += 1;
      const seq = editSeq;
      if (timer) {
        clearTimeout(timer);
      }
      timer = setTimeout(() => {
        timer = null;
        runCompile(project, seq);
      }, debounceMs);
    };

    // Compile once on mount for the current document, then on every project change.
    schedule(useDocumentStore.getState().project);
    const unsubscribe = useDocumentStore.subscribe((state, prev) => {
      if (state.project !== prev.project) {
        schedule(state.project);
      }
    });

    return () => {
      disposed = true;
      if (timer) {
        clearTimeout(timer);
      }
      unsubscribe();
    };
  }, [debounceMs]);
}
