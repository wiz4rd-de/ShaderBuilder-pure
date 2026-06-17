//! `core-model` — the single shared serialization contract.
//!
//! The `Project / Pass / Graph / Node / Parameter` model is defined here once as
//! Rust `serde` types, and the matching TypeScript is **generated** from these
//! types (via `ts-rs`). The *same* schema is used for three things so they can
//! never drift (Architecture §A):
//!
//! 1. **IPC** — Tauri command/event payloads between Rust and the web UI.
//! 2. **The native project file** — JSON on disk (Spec §6).
//! 3. **Import / export** — the in-memory model a `.slangp` maps to.
//!
//! ## Conventions
//!
//! - All fields serialize as **`camelCase`** (`#[serde(rename_all = "camelCase")]`)
//!   so the JSON reads naturally in TypeScript.
//! - Tagged unions use an internal **`"kind"`** discriminator
//!   (`#[serde(tag = "kind")]`), which `ts-rs` turns into a discriminated union.
//! - This is a **skeleton**: the fields are real but minimal. The full node
//!   taxonomy and typed ports arrive with the editor in Phase 5; this only fixes
//!   the shape and the generation pipeline.
//!
//! ## Regenerating the TypeScript bindings
//!
//! ```text
//! cargo test -p core-model        # writes web/src/bindings/*.ts
//! ```
//!
//! `ts-rs`'s `#[ts(export)]` emits a test per type that writes the binding when
//! `cargo test` runs; the output directory is `TS_RS_EXPORT_DIR`, set to
//! `web/src/bindings` in the workspace `.cargo/config.toml`. CI regenerates and
//! fails on any diff, so the committed bindings can never drift from the Rust
//! source of truth.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Crate identity marker. Kept from the Phase 0 scaffold so the inter-crate
/// dependency edges stay referenced; see the workspace stub crates.
pub const NAME: &str = "core-model";

/// Current version of the on-disk project schema. Bump on any breaking change so
/// old project files can be detected and migrated.
pub const PROJECT_SCHEMA_VERSION: u32 = 1;

/// A ShaderBuilder project — the native project file (Spec §6). Distinct from an
/// exported `.slangp` bundle; this is the editable document the frontend owns
/// (Architecture §A).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Project {
    /// Schema version of this project file; see [`PROJECT_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Human-readable project name.
    pub name: String,
    /// The render pipeline: an ordered list of passes. Maps 1:1 to a `.slangp`
    /// (Spec §3, pipeline view).
    pub passes: Vec<Pass>,
    /// `feedback_pass` — the global pass index double-buffered for feedback, or
    /// `None` when the preset declares no global feedback pass (RetroArch default
    /// `-1`). Distinct from per-pass `<alias>Feedback` bindings (§4). Carried on
    /// the project so import → re-export is lossless.
    #[serde(default)]
    pub feedback_pass: Option<u32>,
}

/// One render pass — exactly one fragment shader (Spec §3). A pass is authored
/// either as a node [`Graph`] or supplied as opaque whole-pass code (e.g. from
/// preset import).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Pass {
    /// Stable unique id (used by the pipeline view and to map diagnostics back).
    pub id: String,
    /// Display name / alias for the pass.
    pub name: String,
    /// What produces this pass's fragment shader.
    pub source: PassSource,
    /// `#pragma parameter` knobs this pass exposes as live sliders (Spec §4).
    pub parameters: Vec<Parameter>,
    /// RetroArch per-pass render settings (scale, format, sampler, feedback;
    /// `docs/retroarch-slang-runtime.md` §1–§3). Defaults to [`PassSettings::default`]
    /// (all `None`) for graph-authored passes that haven't set anything yet.
    #[serde(default)]
    pub settings: PassSettings,
}

/// How a pass's render target (FBO) size is derived from the available size
/// inputs (`docs/retroarch-slang-runtime.md` §2). The serde tag is the exact
/// RetroArch `.slangp` string, so import → export round-trips losslessly.
///
/// The librashader-only `Original` extension has no upstream preset string and
/// is intentionally **not** represented here (§11 open-question 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ScaleType {
    /// `source`: factor × this pass's input size (`OriginalSize` for pass 0,
    /// else the previous FBO size). RetroArch `RARCH_SCALE_INPUT`.
    Source,
    /// `viewport`: factor × the simulated final viewport size
    /// (`FinalViewportSize`). RetroArch `RARCH_SCALE_VIEWPORT`.
    Viewport,
    /// `absolute`: a literal integer pixel count; the input size is ignored.
    /// RetroArch `RARCH_SCALE_ABSOLUTE`.
    Absolute,
}

/// Sampler wrap mode for a pass's (or LUT's) source texture
/// (`docs/retroarch-slang-runtime.md` §3, `video_shader_wrap_str_to_mode`). The
/// serde representation is camelCase (e.g. `"clampToBorder"`) per the §A
/// convention; the v1 default is [`WrapMode::ClampToBorder`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum WrapMode {
    /// `clamp_to_border` — RetroArch `RARCH_WRAP_BORDER`, the default.
    ClampToBorder,
    /// `clamp_to_edge` — RetroArch `RARCH_WRAP_EDGE`.
    ClampToEdge,
    /// `repeat` — RetroArch `RARCH_WRAP_REPEAT`.
    Repeat,
    /// `mirrored_repeat` — RetroArch `RARCH_WRAP_MIRRORED_REPEAT`.
    MirroredRepeat,
}

/// One axis of a pass's scale specification: a scale type and its factor
/// (`docs/retroarch-slang-runtime.md` §2). Each field is `Option` so an absent
/// preset key stays absent — the engine applies the position-dependent default
/// (intermediate = `source × 1.0`, final = `viewport`) rather than this carrying
/// an invented value.
///
/// For `Absolute` the `scale` is the literal integer pixel count (stored as
/// `f32`; callers round). For `Source`/`Viewport` it multiplies the relevant
/// size.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ScaleAxis {
    /// The effective scale type for this axis, or `None` if the preset set no
    /// scale-type key for it.
    #[serde(default)]
    pub scale_type: Option<ScaleType>,
    /// The effective scale factor for this axis, or `None` if the preset set no
    /// scale-factor key for it.
    #[serde(default)]
    pub scale: Option<f32>,
}

/// RetroArch per-pass render settings carried on a [`Pass`]
/// (`docs/retroarch-slang-runtime.md` §1–§3). Every field is optional: `None`
/// means "the preset did not set this key" so the engine can apply the
/// position-dependent §2/§3 defaults rather than this baking one in.
///
/// ## Combined-vs-per-axis scale precedence (§2)
///
/// The `.slangp` file may set scale either combined (`scale_typeN` / `scaleN`,
/// applying to both axes) or per-axis (`scale_type_xN` / `scale_xN` and the `_y`
/// forms). Per [`preset_io::Pass::scale_type_x`] et al., the per-axis key wins
/// over the combined key for its axis, and a combined key applies to whichever
/// axis has no per-axis override. The import bridge ([`preset_io::import_preset`])
/// **resolves** that precedence and stores the already-effective per-axis values
/// in [`ScaleAxis`], so this model never carries the raw combined/per-axis
/// ambiguity — `scale_x`/`scale_y` here are the final values.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PassSettings {
    /// Effective X-axis scale (combined/per-axis precedence already resolved).
    #[serde(default)]
    pub scale_x: ScaleAxis,
    /// Effective Y-axis scale (combined/per-axis precedence already resolved).
    #[serde(default)]
    pub scale_y: ScaleAxis,
    /// `filter_linearN` — `true`=linear, `false`=nearest input filtering; `None`
    /// ⇒ unspecified (engine uses the §3 v1 default, linear).
    #[serde(default)]
    pub filter_linear: Option<bool>,
    /// `wrap_modeN` — sampler wrap for this pass's input; `None` ⇒ §3 default
    /// (`clampToBorder`).
    #[serde(default)]
    pub wrap_mode: Option<WrapMode>,
    /// `mipmap_inputN` — generate a mip chain for this pass's input texture.
    #[serde(default)]
    pub mipmap_input: Option<bool>,
    /// `float_framebufferN` — `true` → RGBA16F render target (§3).
    #[serde(default)]
    pub float_framebuffer: Option<bool>,
    /// `srgb_framebufferN` — `true` → RGBA8 sRGB render target (§3).
    #[serde(default)]
    pub srgb_framebuffer: Option<bool>,
    /// `aliasN` — semantic name enabling `<alias>` / `<alias>Size` /
    /// `<alias>Feedback` bindings from later passes (§1/§4). Empty in the preset
    /// ⇒ `None`.
    #[serde(default)]
    pub alias: Option<String>,
    /// `frame_count_modN` — if `>0`, the `FrameCount` fed to this pass wraps mod
    /// this value (§1). `None`/`0` ⇒ no wrap.
    #[serde(default)]
    pub frame_count_mod: Option<u32>,
}

/// How a pass's shader is defined (Spec §3, Architecture §C).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum PassSource {
    /// A node graph that lowers to one fragment shader.
    Graph {
        /// The per-pass node graph.
        graph: Graph,
    },
    /// Opaque whole-pass slang source taken verbatim — the escape hatch, and
    /// what preset import produces (Spec §3/§5).
    WholePassCode {
        /// The complete `.slang` pass source.
        source: String,
    },
}

/// A per-pass node graph — a typed dataflow DAG (Architecture §C). Skeletal
/// here; the node taxonomy and typed ports arrive with the editor in Phase 5.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Graph {
    /// The nodes in this graph.
    pub nodes: Vec<Node>,
    /// The directed, port-to-port connections between nodes.
    pub edges: Vec<Edge>,
}

/// Default for [`Node::data`]: an empty JSON object (so an omitted `data`
/// matches the non-nullable generated `Record<string, unknown>` type).
fn empty_object() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

/// A single node in a per-pass [`Graph`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Node {
    /// Stable unique id (referenced by [`Edge`]s and diagnostics).
    pub id: String,
    /// Node type key, e.g. `"sampler.source"`, `"math.mix"`, `"output"`. The
    /// full taxonomy is defined in Phase 5 (Spec §8.3).
    pub kind: String,
    /// Position on the editor canvas.
    pub position: Vec2,
    /// Node configuration. Free-form JSON object until the typed node set lands;
    /// the generated TypeScript types this as `Record<string, unknown>`. Defaults
    /// to `{}` (not `null`) when omitted, so the on-wire value always matches the
    /// non-nullable generated type.
    #[serde(default = "empty_object")]
    #[ts(type = "Record<string, unknown>")]
    pub data: serde_json::Value,
}

/// A directed connection between two node ports (Architecture §C, Spec §8.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Edge {
    /// Stable unique id for this connection.
    pub id: String,
    /// Id of the node the connection starts at.
    pub source: String,
    /// Output port on the source node.
    pub source_port: String,
    /// Id of the node the connection ends at.
    pub target: String,
    /// Input port on the target node.
    pub target_port: String,
}

/// A `#pragma parameter` declaration — a runtime knob shown as a slider (Spec §4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Parameter {
    /// Identifier used in the shader (the `#pragma parameter` name).
    pub name: String,
    /// Human-readable label shown in the slider UI.
    pub label: String,
    /// Default value.
    pub default: f32,
    /// Minimum slider value.
    pub min: f32,
    /// Maximum slider value.
    pub max: f32,
    /// Slider increment.
    pub step: f32,
}

/// A 2D vector, used for editor node positions.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

/// The simulated viewport the final pass renders into (#30, Architecture §D,
/// Spec §4, `docs/retroarch-slang-runtime.md` §2/§9): the output resolution
/// RetroArch would target, with an optional integer-scale toggle.
///
/// This is the resolution `FinalViewportSize` reports and `viewport`-scaled FBOs
/// multiply — distinct from the preview *pane* size (the read-back/stream target).
/// The engine computes the effective content rectangle from this and the source
/// size (aspect-correct fit, or — when `integer_scale` is set — the largest
/// integer multiple of the source that fits), letterboxing the remainder. See
/// `preview_engine::viewport::ViewportConfig` for the canonical math.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Viewport {
    /// Output resolution width in pixels.
    pub width: u32,
    /// Output resolution height in pixels.
    pub height: u32,
    /// When `true`, snap the content rectangle to the largest integer multiple
    /// of the source size that fits the output resolution (letterboxing the
    /// remainder). When `false`, aspect-correct fit preserving the source ratio.
    pub integer_scale: bool,
}

impl Project {
    /// A new, empty project at the current schema version.
    pub fn empty(name: impl Into<String>) -> Self {
        Self {
            schema_version: PROJECT_SCHEMA_VERSION,
            name: name.into(),
            passes: Vec::new(),
            feedback_pass: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_marker() {
        assert_eq!(NAME, "core-model");
    }

    #[test]
    fn project_json_round_trips() {
        let project = Project {
            schema_version: PROJECT_SCHEMA_VERSION,
            name: "Demo".to_owned(),
            feedback_pass: Some(1),
            passes: vec![
                Pass {
                    id: "pass-0".to_owned(),
                    name: "CRT".to_owned(),
                    source: PassSource::Graph {
                        graph: Graph {
                            nodes: vec![Node {
                                id: "n0".to_owned(),
                                kind: "output".to_owned(),
                                position: Vec2 { x: 1.0, y: 2.0 },
                                data: serde_json::json!({ "note": "skeleton" }),
                            }],
                            edges: vec![],
                        },
                    },
                    parameters: vec![Parameter {
                        name: "BRIGHTNESS".to_owned(),
                        label: "Brightness".to_owned(),
                        default: 1.0,
                        min: 0.0,
                        max: 2.0,
                        step: 0.01,
                    }],
                    settings: PassSettings {
                        scale_x: ScaleAxis {
                            scale_type: Some(ScaleType::Source),
                            scale: Some(2.0),
                        },
                        scale_y: ScaleAxis {
                            scale_type: Some(ScaleType::Source),
                            scale: Some(2.0),
                        },
                        filter_linear: Some(true),
                        wrap_mode: Some(WrapMode::ClampToBorder),
                        mipmap_input: None,
                        float_framebuffer: None,
                        srgb_framebuffer: Some(true),
                        alias: Some("FirstPass".to_owned()),
                        frame_count_mod: Some(60),
                    },
                },
                Pass {
                    id: "pass-1".to_owned(),
                    name: "Imported".to_owned(),
                    source: PassSource::WholePassCode {
                        source: "// verbatim slang".to_owned(),
                    },
                    parameters: vec![],
                    settings: PassSettings::default(),
                },
            ],
        };

        let json = serde_json::to_string_pretty(&project).expect("serialize");
        let back: Project = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(project, back);
    }

    #[test]
    fn node_data_defaults_to_empty_object() {
        // `data` omitted -> {} (a valid Record<string, unknown>), never null.
        let node: Node =
            serde_json::from_str(r#"{"id":"n0","kind":"output","position":{"x":0,"y":0}}"#)
                .expect("deserialize node without data");
        assert_eq!(node.data, serde_json::json!({}));
    }

    #[test]
    fn scale_type_serializes_as_retroarch_strings() {
        // The serde tag must be the exact `.slangp` string so import → export
        // round-trips losslessly (and matches the parser's accepted strings).
        assert_eq!(
            serde_json::to_value(ScaleType::Source).unwrap(),
            serde_json::json!("source")
        );
        assert_eq!(
            serde_json::to_value(ScaleType::Viewport).unwrap(),
            serde_json::json!("viewport")
        );
        assert_eq!(
            serde_json::to_value(ScaleType::Absolute).unwrap(),
            serde_json::json!("absolute")
        );
    }

    #[test]
    fn wrap_mode_serializes_as_camel_case() {
        assert_eq!(
            serde_json::to_value(WrapMode::ClampToBorder).unwrap(),
            serde_json::json!("clampToBorder")
        );
        assert_eq!(
            serde_json::to_value(WrapMode::MirroredRepeat).unwrap(),
            serde_json::json!("mirroredRepeat")
        );
    }

    #[test]
    fn pass_settings_default_is_all_none() {
        let s = PassSettings::default();
        assert_eq!(s.scale_x, ScaleAxis::default());
        assert_eq!(s.scale_y, ScaleAxis::default());
        assert_eq!(s.filter_linear, None);
        assert_eq!(s.alias, None);
        // An omitted `settings`/`feedbackPass` deserializes to the defaults, so
        // older project files (schemaVersion 1, no settings) still load.
        let pass: Pass = serde_json::from_str(
            r#"{"id":"p","name":"n","source":{"kind":"wholePassCode","source":""},"parameters":[]}"#,
        )
        .expect("pass without settings deserializes");
        assert_eq!(pass.settings, PassSettings::default());
    }

    #[test]
    fn pass_source_is_internally_tagged() {
        let graph = PassSource::Graph {
            graph: Graph::default(),
        };
        let value = serde_json::to_value(&graph).unwrap();
        assert_eq!(value["kind"], "graph");

        let code = PassSource::WholePassCode {
            source: "x".to_owned(),
        };
        let value = serde_json::to_value(&code).unwrap();
        assert_eq!(value["kind"], "wholePassCode");
        assert_eq!(value["source"], "x");
    }
}
