// Compare controls for the preview pane (#60): capture/clear a reference frame,
// flip the displayed source between LIVE and the captured REFERENCE (instant), and
// switch to a SPLIT view with a draggable divider. All state is UI-only (the
// compare store) — toggling never marks the project dirty and never blocks the
// render thread.
//
// With NO reference captured, the compare controls (A/B + split + clear) are
// DISABLED and clearly inert rather than erroring (acceptance).

import type { CompareMode } from "./compareGeometry";
import { useCompareStore } from "./compareStore";

/** The three modes, in display order, with their button labels. */
const MODE_LABELS: ReadonlyArray<readonly [CompareMode, string]> = [
  ["live", "Live"],
  ["reference", "Reference"],
  ["split", "Split"],
];

export interface CompareControlsProps {
  /** Whether at least one live frame exists (gates "set reference"). */
  hasLiveFrame: boolean;
  /** Snapshot the current live frame as the reference. */
  onSetReference: () => void;
}

export function CompareControls({ hasLiveFrame, onSetReference }: CompareControlsProps) {
  const mode = useCompareStore((s) => s.mode);
  const orientation = useCompareStore((s) => s.orientation);
  const reference = useCompareStore((s) => s.reference);
  const setMode = useCompareStore((s) => s.setMode);
  const setOrientation = useCompareStore((s) => s.setOrientation);
  const clearReference = useCompareStore((s) => s.clearReference);

  const hasReference = reference !== null;

  return (
    <div className="preview__compare" role="group" aria-label="Compare">
      <button
        type="button"
        className="preview__compare-set"
        onClick={onSetReference}
        disabled={!hasLiveFrame}
        title="Capture the current frame as the reference (A)"
      >
        Set reference
      </button>

      {/* Mode toggle: Live / Reference / Split. Disabled until a reference exists,
          so with nothing captured the controls are inert (acceptance). */}
      <div className="preview__compare-modes" role="radiogroup" aria-label="Compare mode">
        {MODE_LABELS.map(([value, label]) => (
          <button
            key={value}
            type="button"
            role="radio"
            aria-checked={mode === value}
            className={`preview__compare-mode${
              mode === value ? " preview__compare-mode--active" : ""
            }`}
            onClick={() => setMode(value)}
            disabled={value !== "live" && !hasReference}
            title={
              value !== "live" && !hasReference
                ? "Capture a reference first"
                : `Show ${label.toLowerCase()}`
            }
          >
            {label}
          </button>
        ))}
      </div>

      {/* Split-only: flip the divider axis. */}
      {mode === "split" && hasReference ? (
        <button
          type="button"
          className="preview__compare-orient"
          onClick={() =>
            setOrientation(orientation === "vertical" ? "horizontal" : "vertical")
          }
          title="Flip the split divider orientation"
        >
          {orientation === "vertical" ? "Vertical" : "Horizontal"}
        </button>
      ) : null}

      <button
        type="button"
        className="preview__compare-clear"
        onClick={clearReference}
        disabled={!hasReference}
        title="Discard the captured reference"
      >
        Clear
      </button>

      {/* A label of which source is currently shown. */}
      <span className="preview__compare-shown" aria-live="polite">
        {!hasReference
          ? "No reference"
          : mode === "live"
            ? "Showing: Live"
            : mode === "reference"
              ? "Showing: Reference"
              : "Split: Reference │ Live"}
      </span>
    </div>
  );
}
