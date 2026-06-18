// Hook: the live global parameter list (#53). Subscribes to the document store,
// scans every whole-pass code pass for its `#pragma parameter`s (async IPC, cached
// per pass id), and folds everything through `collectParameters` into the single
// de-duplicated knob list the slider panel renders.
//
// Whole-pass scanning is async + impure (Tauri `scan_pass_source`), so it cannot
// live inside the pure `collectParameters`; we scan here in an effect, hold the
// results in state keyed by pass id, and re-collect whenever the project or a scan
// result changes. Graph-pass Param nodes are collected synchronously (pure).
import { useEffect, useMemo, useRef, useState } from "react";

import type { Parameter } from "../bindings/Parameter";
import { useDocumentStore } from "../store/documentStore";
import { scanPassSource } from "../wholePass/scanPassSource";
import { collectParameters } from "./collectParameters";

/** A whole-pass pass's verbatim source, keyed by pass id (for scan invalidation). */
function wholePassSources(passes: ReturnType<typeof passesOf>): Record<string, string> {
  const out: Record<string, string> = {};
  for (const p of passes) {
    if (p.source.kind === "wholePassCode") {
      out[p.id] = p.source.source;
    }
  }
  return out;
}

/** Narrow helper so the source-map builder is typed without importing Pass here. */
function passesOf(
  project: ReturnType<typeof useDocumentStore.getState>["project"],
) {
  return project.passes;
}

/**
 * The global, de-duplicated parameter list for the current document — Param nodes
 * (graph passes) + scanned `#pragma parameter`s (whole-pass passes) + project/pass
 * declarations. Re-scans whole-pass sources (debounced via the store update cadence)
 * and re-collects on any relevant change.
 */
export function useCollectedParameters(): Parameter[] {
  const project = useDocumentStore((s) => s.project);
  const aliases = useMemo(
    () => project.pipeline.aliases.map((a) => a.alias),
    [project.pipeline.aliases],
  );
  const lutNames = useMemo(() => project.luts.map((l) => l.name), [project.luts]);

  // Scanned whole-pass parameters keyed by pass id. Each entry is recomputed when
  // that pass's source string changes.
  const [scanned, setScanned] = useState<Record<string, Parameter[]>>({});
  // The source string we last scanned per pass id (to skip redundant re-scans).
  const lastScanned = useRef<Record<string, string>>({});

  const sources = useMemo(() => wholePassSources(passesOf(project)), [project]);

  useEffect(() => {
    let cancelled = false;
    const ids = Object.keys(sources);
    // Ids whose scan this effect run kicked off but hasn't yet resolved. If this
    // run is cancelled (a dep changed before the scan came back), we roll their
    // lastScanned entry back so the NEXT run retries them — otherwise a pass whose
    // first scan was cancelled would be treated as already-scanned forever and its
    // parameters would silently vanish.
    const inFlight = new Map<string, string>();

    // Drop scan results for passes that are no longer whole-pass code.
    setScanned((prev) => {
      let changed = false;
      const next: Record<string, Parameter[]> = {};
      for (const id of ids) {
        if (id in prev) {
          next[id] = prev[id]!;
        }
      }
      changed = Object.keys(prev).length !== Object.keys(next).length;
      for (const id of Object.keys(lastScanned.current)) {
        if (!(id in sources)) {
          delete lastScanned.current[id];
        }
      }
      return changed ? next : prev;
    });

    // (Re)scan any pass whose source changed since we last scanned it.
    for (const id of ids) {
      const src = sources[id]!;
      if (lastScanned.current[id] === src) {
        continue;
      }
      const prevScanned = lastScanned.current[id];
      lastScanned.current[id] = src;
      inFlight.set(id, src);
      void scanPassSource(src, aliases, lutNames).then(
        (result) => {
          inFlight.delete(id);
          if (cancelled) {
            return;
          }
          setScanned((prev) => ({ ...prev, [id]: result.parameters }));
        },
        () => {
          // A failed scan also leaves no result; allow a retry on the next run.
          inFlight.delete(id);
          if (lastScanned.current[id] === src) {
            if (prevScanned === undefined) {
              delete lastScanned.current[id];
            } else {
              lastScanned.current[id] = prevScanned;
            }
          }
        },
      );
    }

    return () => {
      cancelled = true;
      // Roll back any still-in-flight scans this run started, so a cancelled first
      // scan is retried (its lastScanned guard would otherwise skip it forever).
      for (const [id, src] of inFlight) {
        if (lastScanned.current[id] === src) {
          delete lastScanned.current[id];
        }
      }
    };
  }, [sources, aliases, lutNames]);

  return useMemo(
    () => collectParameters(project, scanned),
    [project, scanned],
  );
}
