// Viewport panel (#48) — drives the SIMULATED output viewport (the resolution
// the chain renders to, distinct from the preview PANE size). It owns a little
// editor-local UI state (resolution, integer-scale, enabled) and pushes it to
// the engine via `set_simulated_viewport({enabled,width,height,integerScale})`,
// matching the Rust signature exactly. `enabled=false` clears the simulated
// viewport (the engine falls back to tracking the pane).
//
// This is intentionally NOT in the document model: the simulated viewport is a
// preview/engine concern, not part of the saved project.
import { invoke } from "@tauri-apps/api/core";
import { useState } from "react";

interface AspectPreset {
  label: string;
  ratio: number | null; // null = free (don't lock height to width)
}

const ASPECTS: AspectPreset[] = [
  { label: "Free", ratio: null },
  { label: "4:3", ratio: 4 / 3 },
  { label: "16:9", ratio: 16 / 9 },
  { label: "1:1", ratio: 1 },
];

/** Derive a height from a width + aspect ratio (rounded to an even-ish int). */
function heightFor(width: number, ratio: number | null, fallback: number): number {
  if (ratio === null || !Number.isFinite(width) || width <= 0) {
    return fallback;
  }
  return Math.max(1, Math.round(width / ratio));
}

export function ViewportPanel(): React.JSX.Element {
  const [enabled, setEnabled] = useState(false);
  const [width, setWidth] = useState(640);
  const [height, setHeight] = useState(480);
  const [aspectIndex, setAspectIndex] = useState(1); // default 4:3
  const [integerScale, setIntegerScale] = useState(false);

  const aspect = ASPECTS[aspectIndex]!;

  /** Push the current state to the engine. */
  const apply = (next: {
    enabled?: boolean;
    width?: number;
    height?: number;
    integerScale?: boolean;
  }) => {
    const e = next.enabled ?? enabled;
    const w = next.width ?? width;
    const h = next.height ?? height;
    const i = next.integerScale ?? integerScale;
    void invoke("set_simulated_viewport", {
      enabled: e,
      width: Math.max(1, Math.round(w)),
      height: Math.max(1, Math.round(h)),
      integerScale: i,
    }).catch((err) => console.error("set_simulated_viewport failed", err));
  };

  const onWidth = (raw: string) => {
    const w = Number(raw);
    if (!Number.isFinite(w)) {
      setWidth(raw === "" ? 0 : width);
      return;
    }
    setWidth(w);
    const h = heightFor(w, aspect.ratio, height);
    if (h !== height) {
      setHeight(h);
    }
    if (enabled) {
      apply({ width: w, height: h });
    }
  };

  const onHeight = (raw: string) => {
    const h = Number(raw);
    if (!Number.isFinite(h)) {
      return;
    }
    setHeight(h);
    if (enabled) {
      apply({ height: h });
    }
  };

  const onAspect = (idx: number) => {
    setAspectIndex(idx);
    const ratio = ASPECTS[idx]!.ratio;
    const h = heightFor(width, ratio, height);
    setHeight(h);
    if (enabled) {
      apply({ height: h });
    }
  };

  const onEnabled = (on: boolean) => {
    setEnabled(on);
    apply({ enabled: on });
  };

  const onIntegerScale = (on: boolean) => {
    setIntegerScale(on);
    if (enabled) {
      apply({ integerScale: on });
    }
  };

  return (
    <div className="panel__body" aria-label="Viewport">
      <label className="panel__field panel__field--inline">
        <input
          type="checkbox"
          aria-label="Simulated viewport enabled"
          checked={enabled}
          onChange={(e) => onEnabled(e.target.checked)}
        />
        <span className="panel__field-label">Simulate output viewport</span>
      </label>

      <div className="panel__field-row">
        <label className="panel__field">
          <span className="panel__field-label">Width</span>
          <input
            type="number"
            className="panel__input panel__input--num"
            aria-label="Viewport width"
            min={1}
            value={width || ""}
            onChange={(e) => onWidth(e.target.value)}
          />
        </label>
        <label className="panel__field">
          <span className="panel__field-label">Height</span>
          <input
            type="number"
            className="panel__input panel__input--num"
            aria-label="Viewport height"
            min={1}
            value={height || ""}
            disabled={aspect.ratio !== null}
            onChange={(e) => onHeight(e.target.value)}
          />
        </label>
      </div>

      <label className="panel__field">
        <span className="panel__field-label">Aspect ratio</span>
        <select
          className="panel__input"
          aria-label="Aspect ratio"
          value={aspectIndex}
          onChange={(e) => onAspect(Number(e.target.value))}
        >
          {ASPECTS.map((a, i) => (
            <option key={a.label} value={i}>
              {a.label}
            </option>
          ))}
        </select>
      </label>

      <label className="panel__field panel__field--inline">
        <input
          type="checkbox"
          aria-label="Integer scale"
          checked={integerScale}
          onChange={(e) => onIntegerScale(e.target.checked)}
        />
        <span className="panel__field-label">Integer scale</span>
      </label>
    </div>
  );
}
