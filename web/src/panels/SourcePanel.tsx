// Source panel (#48) — loads the preview input source and drives its transport.
//
// Sources (all existing Phase-2 IPC commands):
//   * built-in test patterns (load_test_pattern) — need NO path, so they
//     exercise the panel without a file picker;
//   * a still image / shader source path (load_source) — needs a path;
//   * a PNG-sequence directory (load_source_sequence) — needs a dir path.
//
// Transport: play / pause / step / seek({index}) / set_fps({fps}). The backend
// emits a "source-position" event { index, len } as it advances; we listen and
// mirror it into the scrubber + frame counter so the UI stays in sync with the
// engine rather than guessing.
//
// File picking: the Tauri dialog plugin is NOT wired in this build, so the
// image/sequence paths are plain text inputs for v1 (the test patterns need no
// path). Swap to a native picker when the dialog plugin lands.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";

interface SourcePosition {
  index: number;
  len: number;
}

const TEST_PATTERNS = [
  { value: "smpte_bars", label: "SMPTE bars" },
  { value: "checkerboard", label: "Checkerboard" },
  { value: "gradient", label: "Gradient" },
  { value: "motion_sweep", label: "Motion sweep" },
] as const;

export function SourcePanel(): React.JSX.Element {
  const [pattern, setPattern] = useState<string>("smpte_bars");
  const [imagePath, setImagePath] = useState("");
  const [seqDir, setSeqDir] = useState("");
  const [fps, setFps] = useState(60);
  const [position, setPosition] = useState<SourcePosition>({ index: 0, len: 1 });
  const [playing, setPlaying] = useState(false);

  // Mirror the backend's source-position event into the transport UI.
  useEffect(() => {
    const unlisten = listen<SourcePosition>("source-position", (event) => {
      setPosition(event.payload);
    });
    return () => {
      void unlisten.then((off) => off());
    };
  }, []);

  const call = (cmd: string, args?: Record<string, unknown>) =>
    void invoke(cmd, args).catch((err) => console.error(`${cmd} failed`, err));

  const loadPattern = () => call("load_test_pattern", { pattern });
  const loadImage = () => call("load_source", { sourcePath: imagePath || null });
  const loadSequence = () => {
    if (seqDir.trim() === "") {
      return;
    }
    call("load_source_sequence", { dir: seqDir });
  };

  const onPlay = () => {
    setPlaying(true);
    call("play");
  };
  const onPause = () => {
    setPlaying(false);
    call("pause");
  };
  const onStep = () => {
    setPlaying(false);
    call("step");
  };
  const onSeek = (index: number) => {
    setPosition((p) => ({ ...p, index }));
    call("seek", { index });
  };
  const onFps = (next: number) => {
    setFps(next);
    if (Number.isFinite(next) && next > 0) {
      call("set_fps", { fps: next });
    }
  };

  const hasSequence = position.len > 1;

  return (
    <div className="panel__body" aria-label="Source">
      {/* ---- Source selection ---- */}
      <fieldset className="panel__group">
        <legend>Test pattern</legend>
        <div className="panel__field-row">
          <select
            className="panel__input"
            aria-label="Test pattern"
            value={pattern}
            onChange={(e) => setPattern(e.target.value)}
          >
            {TEST_PATTERNS.map((p) => (
              <option key={p.value} value={p.value}>
                {p.label}
              </option>
            ))}
          </select>
          <button type="button" className="panel__btn" onClick={loadPattern}>
            Load
          </button>
        </div>
      </fieldset>

      <fieldset className="panel__group">
        <legend>Image</legend>
        <div className="panel__field-row">
          <input
            type="text"
            className="panel__input"
            aria-label="Image path"
            placeholder="/path/to/image.png (empty = built-in)"
            value={imagePath}
            onChange={(e) => setImagePath(e.target.value)}
          />
          <button type="button" className="panel__btn" onClick={loadImage}>
            Load
          </button>
        </div>
      </fieldset>

      <fieldset className="panel__group">
        <legend>PNG sequence</legend>
        <div className="panel__field-row">
          <input
            type="text"
            className="panel__input"
            aria-label="Sequence directory"
            placeholder="/path/to/frames/"
            value={seqDir}
            onChange={(e) => setSeqDir(e.target.value)}
          />
          <button
            type="button"
            className="panel__btn"
            onClick={loadSequence}
            disabled={seqDir.trim() === ""}
          >
            Load
          </button>
        </div>
      </fieldset>

      {/* ---- Transport ---- */}
      <fieldset className="panel__group">
        <legend>Transport</legend>
        <div className="panel__transport">
          {playing ? (
            <button type="button" className="panel__btn" onClick={onPause}>
              Pause
            </button>
          ) : (
            <button type="button" className="panel__btn" onClick={onPlay}>
              Play
            </button>
          )}
          <button type="button" className="panel__btn" onClick={onStep}>
            Step
          </button>
          <span className="panel__frame-counter" aria-label="Frame counter">
            {position.index} / {Math.max(0, position.len - 1)}
          </span>
        </div>

        <label className="panel__field">
          <span className="panel__field-label">Frame</span>
          <input
            type="range"
            aria-label="Seek"
            min={0}
            max={Math.max(0, position.len - 1)}
            value={position.index}
            disabled={!hasSequence}
            onChange={(e) => onSeek(Number(e.target.value))}
          />
        </label>

        <label className="panel__field">
          <span className="panel__field-label">FPS</span>
          <input
            type="number"
            className="panel__input panel__input--num"
            aria-label="FPS"
            min={1}
            value={fps}
            onChange={(e) => onFps(Number(e.target.value))}
          />
        </label>
      </fieldset>
    </div>
  );
}
