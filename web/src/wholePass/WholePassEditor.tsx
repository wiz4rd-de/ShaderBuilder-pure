// The PASS-LEVEL whole-pass code editor (#52). When the active pass is opaque
// `WholePassCode` (a verbatim `.slang` body — authored here OR produced by a
// Phase-3 preset import), the pass-level canvas shows THIS editor instead of the
// React Flow node graph. It is the editing surface for an opaque pass:
//
//   * a monospace code editor over the verbatim source (coalesced live edits →
//     one undo entry per typing burst, via the store's patchWholePassSource);
//   * a "Convert to node graph" action (replaces the opaque source with an empty
//     graph — distinct from a snippet node, which lives INSIDE a graph);
//   * a read-only summary of the `#pragma parameter`s + RetroArch texture refs the
//     body declares, recovered by the Rust `scan_pass_source` command (reusing the
//     Phase-3 import scanners) — what drives the sliders (#53) + pipeline view (#46).
import { useEffect, useRef, useState } from "react";

import type { Pass } from "../bindings/Pass";
import { useDocumentStore } from "../store/documentStore";
import { scanPassSource, type ScanPassResult } from "./scanPassSource";

/** Pull the verbatim source out of a whole-pass code pass (empty otherwise). */
function sourceOf(pass: Pass): string {
  return pass.source.kind === "wholePassCode" ? pass.source.source : "";
}

/** The filename a whole-pass pass came from (import), or null for authored ones. */
function filenameOf(pass: Pass): string | null {
  return pass.source.kind === "wholePassCode" ? pass.source.filename : null;
}

export function WholePassEditor(): React.JSX.Element {
  const activePassId = useDocumentStore((s) => s.activePassId);
  const pass = useDocumentStore((s) =>
    s.project.passes.find((p) => p.id === activePassId),
  );
  // Project-level aliases + LUT names so the scanner classifies referenced
  // aliases (a bare graph pass has none; an imported preset carries them).
  const aliases = useDocumentStore((s) => s.project.pipeline.aliases.map((a) => a.alias));
  const lutNames = useDocumentStore((s) => s.project.luts.map((l) => l.name));

  const beginInteraction = useDocumentStore((s) => s.beginInteraction);
  const commit = useDocumentStore((s) => s.commit);
  const patchWholePassSource = useDocumentStore((s) => s.patchWholePassSource);
  const setPassToGraph = useDocumentStore((s) => s.setPassToGraph);

  const storedSource = pass ? sourceOf(pass) : "";
  const [text, setText] = useState(storedSource);
  // Resync the editor when the underlying source changes externally (undo/redo,
  // pass switch, import) — but NOT on our own keystrokes (text already leads).
  useEffect(() => setText(storedSource), [storedSource]);

  const [scan, setScan] = useState<ScanPassResult>({ parameters: [], references: [] });
  // Debounced scan: re-scan the source string a beat after typing settles.
  const scanTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    if (scanTimer.current !== null) {
      clearTimeout(scanTimer.current);
    }
    scanTimer.current = setTimeout(() => {
      let cancelled = false;
      void scanPassSource(text, aliases, lutNames).then((result) => {
        if (!cancelled) {
          setScan(result);
        }
      });
      return () => {
        cancelled = true;
      };
    }, 250);
    return () => {
      if (scanTimer.current !== null) {
        clearTimeout(scanTimer.current);
      }
    };
  }, [text, aliases, lutNames]);

  // Coalesced edit: open an interaction on the first keystroke, commit on blur.
  const interacting = useRef(false);
  function onChange(value: string): void {
    if (!pass) {
      return;
    }
    if (!interacting.current) {
      interacting.current = true;
      beginInteraction();
    }
    setText(value);
    patchWholePassSource(pass.id, value);
  }
  function onBlur(): void {
    if (interacting.current) {
      interacting.current = false;
      commit();
    }
  }

  if (!pass || pass.source.kind !== "wholePassCode") {
    return (
      <div className="whole-pass">
        <div className="whole-pass__placeholder">This pass is not a whole-pass code pass.</div>
      </div>
    );
  }

  const filename = filenameOf(pass);

  return (
    <div className="whole-pass" aria-label="Whole-pass code editor">
      <div className="whole-pass__toolbar">
        <span className="whole-pass__badge">whole-pass code</span>
        {filename ? <span className="whole-pass__filename">{filename}</span> : null}
        <span className="whole-pass__spacer" />
        <button
          type="button"
          className="whole-pass__convert"
          onClick={() => setPassToGraph(pass.id)}
          title="Replace the opaque source with an empty node graph"
        >
          Convert to node graph
        </button>
      </div>

      <textarea
        className="whole-pass__code"
        aria-label="Pass source"
        spellCheck={false}
        value={text}
        onChange={(e) => onChange(e.target.value)}
        onBlur={onBlur}
      />

      <div className="whole-pass__scan" aria-label="Declared parameters and references">
        <div className="whole-pass__scan-group">
          <div className="whole-pass__scan-title">
            Parameters ({scan.parameters.length})
          </div>
          {scan.parameters.length === 0 ? (
            <div className="whole-pass__scan-empty">none declared</div>
          ) : (
            scan.parameters.map((p) => (
              <span key={p.name} className="whole-pass__chip whole-pass__chip--param">
                {p.name}
              </span>
            ))
          )}
        </div>
        <div className="whole-pass__scan-group">
          <div className="whole-pass__scan-title">
            Texture references ({scan.references.length})
          </div>
          {scan.references.length === 0 ? (
            <div className="whole-pass__scan-empty">none referenced</div>
          ) : (
            scan.references.map((r) => (
              <span
                key={r.name}
                className={`whole-pass__chip whole-pass__chip--ref whole-pass__chip--${r.kind}`}
                title={r.kind}
              >
                {r.name}
              </span>
            ))
          )}
        </div>
      </div>
    </div>
  );
}
