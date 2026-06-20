// Pixel-inspector controls for the preview pane (#61, Spec §8.5): toggle the
// inspector on/off, switch the readout display (0-255 vs 0-1, sRGB vs linear), and
// clear all pinned samples. The display toggles change only the DISPLAYED value,
// never the readback. All state is UI-only (the inspector store) — never dirties
// the project.

import { useInspectorStore } from "./inspectorStore";

export function InspectorControls() {
  const enabled = useInspectorStore((s) => s.enabled);
  const display = useInspectorStore((s) => s.display);
  const pinnedCount = useInspectorStore((s) => s.pinned.length);
  const setEnabled = useInspectorStore((s) => s.setEnabled);
  const setBytes = useInspectorStore((s) => s.setBytes);
  const setSrgb = useInspectorStore((s) => s.setSrgb);
  const clearPins = useInspectorStore((s) => s.clearPins);

  return (
    <div className="preview__inspect-controls" role="group" aria-label="Pixel inspector">
      <button
        type="button"
        className={`preview__inspect-toggle${enabled ? " preview__inspect-toggle--active" : ""}`}
        aria-pressed={enabled}
        onClick={() => setEnabled(!enabled)}
        title="Hover the preview to read a pixel's viewport coordinate + RGBA"
      >
        Inspect
      </button>

      {/* Display toggles: only meaningful with the inspector on or pins present. */}
      <div
        className="preview__inspect-units"
        role="radiogroup"
        aria-label="Value units"
      >
        <button
          type="button"
          role="radio"
          aria-checked={display.bytes}
          className={`preview__inspect-unit${display.bytes ? " preview__inspect-unit--active" : ""}`}
          onClick={() => setBytes(true)}
          title="Show channels as 0–255"
        >
          0–255
        </button>
        <button
          type="button"
          role="radio"
          aria-checked={!display.bytes}
          className={`preview__inspect-unit${!display.bytes ? " preview__inspect-unit--active" : ""}`}
          onClick={() => setBytes(false)}
          title="Show channels as 0–1 floats"
        >
          0–1
        </button>
      </div>

      <div
        className="preview__inspect-units"
        role="radiogroup"
        aria-label="Color space"
      >
        <button
          type="button"
          role="radio"
          aria-checked={!display.srgb}
          className={`preview__inspect-unit${!display.srgb ? " preview__inspect-unit--active" : ""}`}
          onClick={() => setSrgb(false)}
          title="Show the raw linear value"
        >
          Linear
        </button>
        <button
          type="button"
          role="radio"
          aria-checked={display.srgb}
          className={`preview__inspect-unit${display.srgb ? " preview__inspect-unit--active" : ""}`}
          onClick={() => setSrgb(true)}
          title="Show the sRGB-encoded value"
        >
          sRGB
        </button>
      </div>

      <button
        type="button"
        className="preview__inspect-clear"
        onClick={clearPins}
        disabled={pinnedCount === 0}
        title="Remove all pinned samples"
      >
        Clear pins{pinnedCount > 0 ? ` (${pinnedCount})` : ""}
      </button>
    </div>
  );
}
