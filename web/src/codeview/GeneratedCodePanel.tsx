// The read-only GENERATED-CODE viewer (#55) — shows the slang `compile_graph`
// emitted for the ACTIVE pass, with light syntax highlighting + a copy button.
//
// OUTPUT-ONLY (Decision Log #5): the generated full-source is NEVER re-parsed back
// into nodes. The viewer therefore presents the source read-only and labels it as
// output that is not editable / not round-tripped. It reads the per-pass source the
// live compile loop (#54) stashed in the store — the SAME source the live preview
// ran and the exporter embeds, so what you see here is exactly the exported
// per-pass `.slang`.
//
// STALE / EMPTY STATE: when the active pass currently fails to compile (its
// `current` source is `null`) the viewer shows the LAST-GOOD source under an
// explicit "stale" banner, or — if the pass never compiled — an empty marker. It
// never silently shows misleading source.
import { useMemo, useState } from "react";

import { useDocumentStore } from "../store/documentStore";
import { tokenizeSlang } from "./highlightSlang";

/** Render highlighted slang into a <pre> of classed <span>s (read-only). */
function HighlightedCode({ source }: { source: string }): React.JSX.Element {
  const tokens = useMemo(() => tokenizeSlang(source), [source]);
  return (
    <pre className="codeview__pre" aria-label="Generated slang" tabIndex={0}>
      <code className="codeview__code">
        {tokens.map((t, i) => (
          <span key={i} className={`tok tok--${t.type}`}>
            {t.text}
          </span>
        ))}
      </code>
    </pre>
  );
}

export function GeneratedCodePanel(): React.JSX.Element {
  const activePassId = useDocumentStore((s) => s.activePassId);
  const passes = useDocumentStore((s) => s.project.passes);
  const sourceState = useDocumentStore((s) => s.sourcesByPassId[activePassId]);
  const compiling = useDocumentStore((s) => s.compiling);

  const [copied, setCopied] = useState(false);

  const pass = passes.find((p) => p.id === activePassId);
  const isWholePass = pass?.source.kind === "wholePassCode";

  // The source to DISPLAY: the current compile's source when present, else the
  // last-good source (shown as stale). `null`/absent → nothing to show yet.
  const current = sourceState?.current ?? null;
  const lastGood = sourceState?.lastGood ?? null;
  const shown = current ?? lastGood;
  const isStale = current === null && lastGood !== null;

  const onCopy = (): void => {
    if (shown == null) {
      return;
    }
    void navigator.clipboard?.writeText(shown).then(
      () => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1200);
      },
      () => {
        /* clipboard denied — leave the button label unchanged */
      },
    );
  };

  return (
    <div className="panel__body codeview" aria-label="Generated code">
      <div className="codeview__header">
        <span className="codeview__title">Generated slang</span>
        <span className="codeview__readonly" title="Output-only: not re-parsed into nodes">
          read-only · output-only
        </span>
        <button
          type="button"
          className="panel__btn codeview__copy"
          onClick={onCopy}
          disabled={shown == null}
        >
          {copied ? "Copied" : "Copy"}
        </button>
      </div>

      <p className="codeview__note">
        This is the source generated for the active pass. It is output-only and is
        not parsed back into nodes; edit the graph to change it.
      </p>

      {isStale ? (
        <div className="codeview__banner codeview__banner--stale" role="status">
          This pass does not currently compile — showing the last source that did.
        </div>
      ) : null}

      {shown != null ? (
        <HighlightedCode source={shown} />
      ) : (
        <div className="panel__placeholder">
          {isWholePass
            ? "This is a whole-pass code pass — its source is authored directly in the code editor, not generated."
            : compiling
              ? "Compiling…"
              : "No generated source yet — this pass has not compiled. Fix any problems, then it will appear here."}
        </div>
      )}
    </div>
  );
}
