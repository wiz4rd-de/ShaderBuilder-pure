// Pass-settings panel (#48) — binds to the ACTIVE pass's `Pass.settings` in the
// document store and writes edits straight back through `updatePassSettings`
// (and `setFeedbackPass` for the project-global feedback toggle). Every control
// is a thin reflection of the core-model `PassSettings` schema; there is NO
// engine state duplicated here — the live compile loop (#54) rebuilds the chain
// from the document, so these edits take effect once the chain is rebuilt.
//
// A "Scale type" of "(default)" maps to `scaleType: null` (the engine applies
// the §2 position-dependent default); choosing source/viewport/absolute writes
// the explicit value. FBO format is a single select that maps onto the
// mutually-exclusive float/srgb framebuffer flags.
import type { PassSettings } from "../bindings/PassSettings";
import type { ScaleAxis } from "../bindings/ScaleAxis";
import type { ScaleType } from "../bindings/ScaleType";
import type { WrapMode } from "../bindings/WrapMode";
import { useDocumentStore } from "../store/documentStore";

/**
 * The starter source a freshly-switched whole-pass code pass (#52) carries: a
 * minimal RetroArch slang pass that samples Source and passes it through. The
 * author edits it in the pass-level code editor; it compiles + previews as-is.
 */
const WHOLE_PASS_TEMPLATE = `#version 450

layout(push_constant) uniform Push {
    vec4 SourceSize;
    vec4 OriginalSize;
    vec4 OutputSize;
    uint FrameCount;
} params;

#pragma stage vertex
layout(location = 0) in vec4 Position;
layout(location = 1) in vec2 TexCoord;
layout(location = 0) out vec2 vTexCoord;

void main() {
    gl_Position = Position;
    vTexCoord = TexCoord;
}

#pragma stage fragment
layout(location = 0) in vec2 vTexCoord;
layout(location = 0) out vec4 FragColor;
layout(set = 0, binding = 2) uniform sampler2D Source;

void main() {
    FragColor = texture(Source, vTexCoord);
}
`;

/** The synthetic FBO-format choices the two boolean flags collapse into. */
type FboFormat = "rgba8" | "float16" | "srgb";

function fboFormatOf(settings: PassSettings): FboFormat {
  if (settings.floatFramebuffer) {
    return "float16";
  }
  if (settings.srgbFramebuffer) {
    return "srgb";
  }
  return "rgba8";
}

/** The scale-type <select> value: "" means "unset / engine default". */
function scaleTypeValue(axis: ScaleAxis): string {
  return axis.scaleType ?? "";
}

export function PassSettingsPanel(): React.JSX.Element {
  const activePassId = useDocumentStore((s) => s.activePassId);
  const passes = useDocumentStore((s) => s.project.passes);
  const feedbackPass = useDocumentStore((s) => s.project.feedbackPass);
  const updatePassSettings = useDocumentStore((s) => s.updatePassSettings);
  const setFeedbackPass = useDocumentStore((s) => s.setFeedbackPass);
  const setPassToWholePassCode = useDocumentStore((s) => s.setPassToWholePassCode);
  const setPassToGraph = useDocumentStore((s) => s.setPassToGraph);

  const passIndex = passes.findIndex((p) => p.id === activePassId);
  const pass = passIndex >= 0 ? passes[passIndex]! : null;

  if (!pass) {
    return (
      <div className="panel__placeholder">No active pass — add or select a pass.</div>
    );
  }

  const s = pass.settings;
  const patch = (p: Partial<PassSettings>) => updatePassSettings(pass.id, p);

  /** Edit one axis of the scale spec, preserving the other field. */
  const setScale = (axis: "scaleX" | "scaleY", next: Partial<ScaleAxis>) => {
    patch({ [axis]: { ...s[axis], ...next } } as Partial<PassSettings>);
  };

  const onScaleType = (axis: "scaleX" | "scaleY", raw: string) => {
    const scaleType = (raw === "" ? null : (raw as ScaleType));
    // Clearing the type clears the factor too (back to fully-unset).
    setScale(axis, scaleType === null ? { scaleType: null, scale: null } : { scaleType });
  };

  const onScaleFactor = (axis: "scaleX" | "scaleY", raw: string) => {
    const n = raw.trim() === "" ? null : Number(raw);
    setScale(axis, { scale: n !== null && Number.isFinite(n) ? n : null });
  };

  const onFboFormat = (fmt: FboFormat) => {
    patch({
      floatFramebuffer: fmt === "float16" ? true : null,
      srgbFramebuffer: fmt === "srgb" ? true : null,
    });
  };

  /** A tri-state boolean: "" = unset (null), "on" = true, "off" = false. */
  const triValue = (v: boolean | null): string => (v === null ? "" : v ? "on" : "off");
  const triParse = (raw: string): boolean | null =>
    raw === "" ? null : raw === "on";

  const onSourceKind = (kind: string) => {
    if (kind === "wholePassCode" && pass.source.kind !== "wholePassCode") {
      setPassToWholePassCode(pass.id, WHOLE_PASS_TEMPLATE);
    } else if (kind === "graph" && pass.source.kind !== "graph") {
      setPassToGraph(pass.id);
    }
  };

  return (
    <div className="panel__body" aria-label="Pass settings">
      <div className="panel__pass-name">{pass.name}</div>

      {/* ---- Source kind (#52): node graph vs opaque whole-pass code ---- */}
      <fieldset className="panel__group">
        <legend>Source</legend>
        <label className="panel__field">
          <span className="panel__field-label">Authoring</span>
          <select
            className="panel__input"
            aria-label="Pass source kind"
            value={pass.source.kind}
            onChange={(e) => onSourceKind(e.target.value)}
          >
            <option value="graph">Node graph</option>
            <option value="wholePassCode">Whole-pass code</option>
          </select>
        </label>
        {pass.source.kind === "wholePassCode" ? (
          <div className="panel__hint">
            Opaque .slang source — edit it in the pass canvas.
          </div>
        ) : null}
      </fieldset>

      {/* ---- Scale ---- */}
      <fieldset className="panel__group">
        <legend>Scale</legend>
        {(["scaleX", "scaleY"] as const).map((axis) => (
          <label className="panel__field" key={axis}>
            <span className="panel__field-label">{axis === "scaleX" ? "X axis" : "Y axis"}</span>
            <div className="panel__field-row">
              <select
                className="panel__input"
                aria-label={`${axis} scale type`}
                value={scaleTypeValue(s[axis])}
                onChange={(e) => onScaleType(axis, e.target.value)}
              >
                <option value="">(default)</option>
                <option value="source">source</option>
                <option value="viewport">viewport</option>
                <option value="absolute">absolute</option>
              </select>
              <input
                type="number"
                className="panel__input panel__input--num"
                aria-label={`${axis} scale factor`}
                placeholder="factor"
                step="any"
                value={s[axis].scale ?? ""}
                disabled={s[axis].scaleType === null}
                onChange={(e) => onScaleFactor(axis, e.target.value)}
              />
            </div>
          </label>
        ))}
      </fieldset>

      {/* ---- Render target ---- */}
      <fieldset className="panel__group">
        <legend>Render target</legend>
        <label className="panel__field">
          <span className="panel__field-label">FBO format</span>
          <select
            className="panel__input"
            aria-label="FBO format"
            value={fboFormatOf(s)}
            onChange={(e) => onFboFormat(e.target.value as FboFormat)}
          >
            <option value="rgba8">rgba8</option>
            <option value="float16">float16</option>
            <option value="srgb">srgb</option>
          </select>
        </label>
        <label className="panel__field">
          <span className="panel__field-label">Filter</span>
          <select
            className="panel__input"
            aria-label="Filter linear"
            value={triValue(s.filterLinear)}
            onChange={(e) => patch({ filterLinear: triParse(e.target.value) })}
          >
            <option value="">(default)</option>
            <option value="on">linear</option>
            <option value="off">nearest</option>
          </select>
        </label>
        <label className="panel__field">
          <span className="panel__field-label">Wrap mode</span>
          <select
            className="panel__input"
            aria-label="Wrap mode"
            value={s.wrapMode ?? ""}
            onChange={(e) =>
              patch({ wrapMode: e.target.value === "" ? null : (e.target.value as WrapMode) })
            }
          >
            <option value="">(default)</option>
            <option value="clampToBorder">clampToBorder</option>
            <option value="clampToEdge">clampToEdge</option>
            <option value="repeat">repeat</option>
            <option value="mirroredRepeat">mirroredRepeat</option>
          </select>
        </label>
        <label className="panel__field panel__field--inline">
          <input
            type="checkbox"
            aria-label="Mipmap input"
            checked={s.mipmapInput === true}
            onChange={(e) => patch({ mipmapInput: e.target.checked ? true : null })}
          />
          <span className="panel__field-label">Mipmap input</span>
        </label>
      </fieldset>

      {/* ---- Identity / feedback ---- */}
      <fieldset className="panel__group">
        <legend>Identity</legend>
        <label className="panel__field">
          <span className="panel__field-label">Alias</span>
          <input
            type="text"
            className="panel__input"
            aria-label="Alias"
            placeholder="(none)"
            value={s.alias ?? ""}
            onChange={(e) =>
              patch({ alias: e.target.value.trim() === "" ? null : e.target.value })
            }
          />
        </label>
        <label className="panel__field panel__field--inline">
          <input
            type="checkbox"
            aria-label="Feedback pass"
            checked={feedbackPass === passIndex}
            onChange={(e) => setFeedbackPass(e.target.checked ? passIndex : null)}
          />
          <span className="panel__field-label">Global feedback pass</span>
        </label>
      </fieldset>
    </div>
  );
}
