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

pub mod ir;

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
    /// Derived pipeline wiring metadata (alias → pass index, per-pass texture
    /// availability) reconstructed at import time (#34). This is *metadata about*
    /// the chain — not a node-IR — so a pass can be referenced by its alias and
    /// the editor knows which RetroArch textures each pass may bind. Defaults to
    /// [`PipelineMetadata::default`] (empty) for hand-built projects.
    #[serde(default)]
    pub pipeline: PipelineMetadata,
    /// The project's runtime parameter knobs — the **reconciled** set of
    /// `#pragma parameter` declarations across every pass, with any `.slangp`
    /// per-parameter overrides applied (#35). RetroArch parameters are global by
    /// id: a parameter declared (identically) in several passes is **one** knob
    /// here. This is the authoritative list the slider UI (#36 export) reads.
    /// Distinct from a [`Pass`]'s own `parameters` (the raw per-pass
    /// declarations). Defaults to `[]` for hand-built projects.
    #[serde(default)]
    pub parameters: Vec<Parameter>,
    /// Imported LUT textures (the `.slangp` `textures` family, §7), with paths
    /// already resolved relative to the preset directory and per-texture sampler
    /// settings carried (#35). Empty for hand-built projects. Defaults to `[]`.
    #[serde(default)]
    pub luts: Vec<Lut>,
    /// Free-form authoring metadata for the native project file (Spec §6, #38):
    /// description, author, timestamps. Every field is optional and the whole
    /// struct defaults to empty, so older project files (and import-produced
    /// projects) load with no metadata. This is **document** metadata; it is
    /// deliberately separate from pipeline wiring (which lives in [`pipeline`]).
    ///
    /// [`pipeline`]: Project::pipeline
    #[serde(default)]
    pub metadata: ProjectMetadata,
    /// References to personal-library items this project instantiated (Spec §6:
    /// "project file holds library refs"; #38, forward-looking for the Phase-6
    /// library, #56/#58). Each entry is a lightweight pointer (id + display name)
    /// to a `LibraryItem` stored elsewhere — the project records *which* library
    /// items it used, never the item bodies. Empty for projects that used no
    /// library items. Defaults to `[]`.
    ///
    /// The full `LibraryItem` schema and the library store land in Phase 6; this
    /// only fixes the reference shape so the native project file can already carry
    /// the refs without round-trip loss.
    #[serde(default)]
    pub library_refs: Vec<LibraryRef>,
}

/// Free-form authoring metadata for the native project file (Spec §6, #38).
///
/// This is **document** metadata — human-facing description, authorship, and
/// timestamps — kept distinct from the render [`PipelineMetadata`] (which is
/// *wiring* metadata about the chain). Every field is optional so a project may
/// carry none of it, and the whole struct defaults to empty so older project
/// files and import-produced projects load unchanged.
///
/// Timestamps are stored as **opaque strings** (RFC 3339 is the intended
/// convention) rather than a date type, so the serde/TS contract stays
/// dependency-free and the frontend owns formatting. The save/load commands do
/// not interpret or mutate these fields — they are authored by the UI.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProjectMetadata {
    /// A longer human-readable description of the project, or `None` if unset.
    #[serde(default)]
    pub description: Option<String>,
    /// The author / creator name, or `None` if unset.
    #[serde(default)]
    pub author: Option<String>,
    /// When the project was first created, as an opaque (RFC 3339) string, or
    /// `None` if unset.
    #[serde(default)]
    pub created_at: Option<String>,
    /// When the project was last modified, as an opaque (RFC 3339) string, or
    /// `None` if unset.
    #[serde(default)]
    pub modified_at: Option<String>,
}

/// A reference from a project to a personal-library item it instantiated (Spec
/// §6: "project file holds library refs"; #38). This is a lightweight **pointer**
/// — the project records *which* library items it used, never the item bodies.
///
/// The full `LibraryItem` schema and the on-disk library store arrive in Phase 6
/// (#56/#58); this fixes only the reference shape so the native project file can
/// already round-trip the refs. Resolving a ref to a concrete library item (and
/// handling a missing/updated item) is a Phase-6 concern.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LibraryRef {
    /// The stable id of the referenced `LibraryItem` (Phase 6). Matches the id the
    /// library store assigns; lets the editor mark instantiated regions and the
    /// library panel show "used by N projects".
    pub item_id: String,
    /// The library item's display name at the time it was instantiated, for
    /// showing a meaningful label even when the item is no longer in the library.
    /// `None` if not captured.
    #[serde(default)]
    pub name: Option<String>,
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
    /// RetroArch textures/aliases this pass's source *textually* references
    /// (`PassOutputN`, `<alias>`, `Original`, `Source`, `…Feedback`, LUT names;
    /// §7). Reconstructed by a **light textual scan** of the whole-pass source at
    /// import time (#34) for pipeline wiring + LUT cross-check — it is **not** a
    /// parse of the pass body into node-IR. Empty for graph-authored passes (and
    /// for an unreadable/empty imported source). Defaults to `[]`.
    #[serde(default)]
    pub references: Vec<TextureRef>,
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
    ///
    /// The pass body is **intentionally NOT decomposed** into a visual node
    /// graph (Architecture §C: whole-pass nodes bypass the node-IR). The
    /// [`WholePassCode::opaque`] marker records that contract on the data itself:
    /// the source is held verbatim and treated as a single non-decomposable unit.
    WholePassCode {
        /// The complete `.slang` pass source, stored **byte-for-byte** as read
        /// from disk on import — no normalization (line endings, trailing
        /// whitespace, BOM) so import → re-export is lossless.
        source: String,
        /// The source `.slang` file name (the `shaderN` basename, e.g.
        /// `"crt-pass1.slang"`), or `None` when the source did not come from a
        /// named file. Carried for display + lossless re-export of the chain.
        #[serde(default)]
        filename: Option<String>,
        /// Marks this source as **opaque / non-decomposable**: its body is taken
        /// verbatim and is *not* lowered into a [`Graph`] of visual nodes
        /// (Architecture §C). Always `true` for whole-pass code; present as an
        /// explicit, serialized contract rather than an implicit convention.
        /// Defaults to `true` so older project files load as opaque.
        #[serde(default = "default_true")]
        opaque: bool,
    },
}

/// Default for [`PassSource::WholePassCode::opaque`]: whole-pass code is opaque.
fn default_true() -> bool {
    true
}

/// One RetroArch texture/alias a whole-pass source textually references (§7),
/// found by the import-time scan (#34). This is *wiring metadata*, deliberately
/// shallow: it records the **name** as written and a coarse [`TextureRefKind`]
/// classification — it does not model where in the body the read occurs, nor
/// does it decompose the pass into node-IR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TextureRef {
    /// The texture/sampler identifier exactly as it appears in the source, e.g.
    /// `"Original"`, `"Source"`, `"PassOutput2"`, `"MyAliasFeedback"`, or a LUT
    /// name like `"BORDER"`.
    pub name: String,
    /// Coarse classification of what `name` refers to (§7 binding table).
    pub kind: TextureRefKind,
}

/// Coarse classification of a [`TextureRef`] (§7 sampler binding table). The
/// import scan classifies by the well-known RetroArch prefixes; anything else is
/// [`TextureRefKind::Alias`] (a `#pragma name` pass alias or a LUT name —
/// resolved against the alias/LUT tables, not distinguished by the scan).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TextureRefKind {
    /// `Original` — the whole-chain input frame (`≡ OriginalHistory0`).
    Original,
    /// `Source` — the previous pass's output (`Original` for pass 0).
    Source,
    /// `PassOutputK` / `PassK` — pass `K`'s output **this frame** (causal).
    PassOutput,
    /// `PassFeedbackK` / `<alias>Feedback` — a pass's output from the previous
    /// frame (§4).
    Feedback,
    /// `OriginalHistoryK` — `Original` from `K` frames ago (§5).
    History,
    /// `UserK` — the un-aliased LUT fallback (§7); a LUT referenced by its alias
    /// name instead is classified as [`TextureRefKind::Alias`].
    User,
    /// A `#pragma name` pass alias or a preset LUT name — distinguished from the
    /// pass/LUT it binds to only by the alias/LUT tables, not by the scan.
    Alias,
}

/// Derived pipeline wiring metadata, reconstructed at import time (#34). This is
/// **metadata about** the rendered chain — ordering, alias bindings, and the
/// per-pass set of bindable RetroArch textures — and is deliberately **not** a
/// node-IR: whole-pass bodies are never decomposed (Architecture §C). It lets a
/// pass be referenced by its `#pragma name` / `aliasN` alias and lets the editor
/// surface what each pass may legally bind without re-scanning sources.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PipelineMetadata {
    /// `alias` → pass index, for every pass that declares an `aliasN`
    /// (`#pragma name`) (§1/§7). Lets a later pass's `<alias>` / `<alias>Feedback`
    /// reference resolve to a concrete pass. Ordered by pass index.
    #[serde(default)]
    pub aliases: Vec<AliasBinding>,
    /// Per-pass availability: for each pass, the set of RetroArch texture
    /// semantic names it may bind (`Original`, `Source`, `PassOutput0..i-1`,
    /// earlier aliases, all LUTs, any feedback). Recorded as metadata so the
    /// editor needn't re-derive causality. Indexed parallel to [`Project::passes`].
    #[serde(default)]
    pub availability: Vec<PassAvailability>,
}

/// One `alias → pass index` binding in [`PipelineMetadata::aliases`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AliasBinding {
    /// The pass's semantic alias (`aliasN` / `#pragma name`).
    pub alias: String,
    /// Index of the pass it names, into [`Project::passes`].
    pub pass_index: u32,
}

/// The set of RetroArch textures a single pass may bind, recorded as pipeline
/// metadata (#34). Causal: only earlier passes' outputs/aliases appear (plus the
/// always-available `Original`/`Source`, all LUTs, and any pass's feedback).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PassAvailability {
    /// Index of the pass this availability is for, into [`Project::passes`].
    pub pass_index: u32,
    /// The semantic texture names bindable from this pass, in a deterministic
    /// order (built-ins, then earlier `PassOutputK`, then earlier aliases, then
    /// LUTs). Names only — sizes (`…Size`) and feedback twins are implied.
    pub available: Vec<String>,
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

/// An imported LUT texture (the `.slangp` `textures` family,
/// `docs/retroarch-slang-runtime.md` §7). A LUT is a static image loaded once and
/// bindable from any pass by its [`Lut::name`] (`<NAME>` / `<NAME>Size`). Carries
/// the per-texture sampler settings RetroArch reads from the `<NAME>_*` keys.
///
/// The `.slangp` per-texture keys are each optional: `None` means the preset did
/// not set the key, so the engine applies the RetroArch LUT default (filtering =
/// nearest, wrap = `clampToBorder`, no mipmaps) rather than this baking one in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Lut {
    /// The LUT name as listed in `textures = "A;B"` and bound as `<NAME>` from a
    /// pass body (§7). Unique within a project.
    pub name: String,
    /// The image path, **already resolved** against the preset directory at import
    /// time (#35) — an absolute path, or a normalized relative path that may point
    /// outside the preset dir (e.g. `../shared/foo.png`). Stored as a string (the
    /// model is the serde/TS contract; the parser uses `PathBuf` internally).
    pub path: String,
    /// `<NAME>_linear` — `true`=linear, `false`=nearest filtering; `None` ⇒ the
    /// preset did not set it (engine default: nearest, §7).
    #[serde(default)]
    pub filter_linear: Option<bool>,
    /// `<NAME>_wrap_mode` — sampler wrap for the LUT; `None` ⇒ unset (engine
    /// default `clampToBorder`).
    #[serde(default)]
    pub wrap_mode: Option<WrapMode>,
    /// `<NAME>_mipmap` — generate a mip chain for the LUT; `None` ⇒ unset
    /// (engine default: no mipmaps).
    #[serde(default)]
    pub mipmap: Option<bool>,
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
            pipeline: PipelineMetadata::default(),
            parameters: Vec::new(),
            luts: Vec::new(),
            metadata: ProjectMetadata::default(),
            library_refs: Vec::new(),
        }
    }

    /// Serialize this project to the native project-file JSON text (Spec §6, #38).
    ///
    /// The output is pretty-printed (stable, diff-friendly, human-inspectable) and
    /// carries [`schema_version`] so a later reader can detect and migrate older
    /// files. This is the **only** serialization the native project file uses; it
    /// never embeds any `.slangp` export concern — exporting a RetroArch bundle is
    /// a wholly separate path ([`crate`]'s consumer `preset_io::export_preset`,
    /// #36). A round trip through [`to_json`]/[`from_json`] reproduces an identical
    /// in-memory model (modulo JSON formatting).
    ///
    /// Serialization is infallible for a well-formed [`Project`] (the model is all
    /// plain serde types), but the signature stays `Result` to surface the
    /// theoretical `serde_json` error rather than panicking.
    ///
    /// [`schema_version`]: Project::schema_version
    /// [`to_json`]: Project::to_json
    /// [`from_json`]: Project::from_json
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Parse native project-file JSON text into a [`Project`] (Spec §6, #38),
    /// applying **versioned** validation so a malformed or out-of-date file yields
    /// a typed [`ProjectLoadError`] rather than a panic.
    ///
    /// Versioning policy (forward-migration ready):
    ///
    /// - The `schema_version` field is read **first**, leniently (only that one
    ///   `u32` field is required to classify the file). A document missing it, or
    ///   one that isn't a JSON object, is [`ProjectLoadError::Malformed`].
    /// - A version **newer** than [`PROJECT_SCHEMA_VERSION`] is
    ///   [`ProjectLoadError::TooNew`] — this build cannot understand it.
    /// - A version **older** than [`PROJECT_SCHEMA_VERSION`] currently has no
    ///   migration registered, so it is [`ProjectLoadError::Unsupported`]. When a
    ///   v2 schema lands, the migration from v1 hooks in here (the `#[serde(default)]`
    ///   on every additive field already keeps same-major loads forward-compatible;
    ///   `Unsupported` is reserved for genuinely breaking version bumps).
    /// - At the current version, the full [`Project`] is deserialized; any shape
    ///   mismatch (a required field absent, a wrong type) is
    ///   [`ProjectLoadError::Malformed`] carrying the `serde_json` message.
    pub fn from_json(json: &str) -> Result<Self, ProjectLoadError> {
        // Read just the version first, tolerating everything else, so an old/new
        // file is classified by version rather than by an incidental shape error.
        #[derive(Deserialize)]
        struct VersionProbe {
            #[serde(rename = "schemaVersion")]
            schema_version: Option<u32>,
        }
        let probe: VersionProbe =
            serde_json::from_str(json).map_err(|e| ProjectLoadError::Malformed {
                message: e.to_string(),
            })?;
        let version = probe
            .schema_version
            .ok_or_else(|| ProjectLoadError::Malformed {
                message: "missing `schemaVersion` field".into(),
            })?;

        match version.cmp(&PROJECT_SCHEMA_VERSION) {
            std::cmp::Ordering::Greater => {
                return Err(ProjectLoadError::TooNew {
                    found: version,
                    supported: PROJECT_SCHEMA_VERSION,
                });
            }
            std::cmp::Ordering::Less => {
                // No migration is registered yet (v1 is the only schema). A future
                // breaking bump installs its v1→vN migration here.
                return Err(ProjectLoadError::Unsupported {
                    found: version,
                    supported: PROJECT_SCHEMA_VERSION,
                });
            }
            std::cmp::Ordering::Equal => {}
        }

        serde_json::from_str(json).map_err(|e| ProjectLoadError::Malformed {
            message: e.to_string(),
        })
    }

    /// Save this project to a single `.json` file at `path` (Spec §6, #38) — the
    /// native project-file writer. Serializes via [`to_json`] and writes the bytes
    /// (a UTF-8 trailing newline is appended so the file is tool-friendly). Any
    /// serialize or write failure is a typed [`ProjectSaveError`], never a panic.
    ///
    /// This writes **one** self-contained JSON document and nothing else: it does
    /// not produce, reference, or touch any `.slangp` bundle. Exporting a RetroArch
    /// bundle is the separate `preset_io::export_preset` path (#36); the boundary
    /// is enforced by these being different functions in different crates.
    ///
    /// [`to_json`]: Project::to_json
    pub fn save_to_file(&self, path: impl AsRef<std::path::Path>) -> Result<(), ProjectSaveError> {
        let mut json = self.to_json().map_err(|e| ProjectSaveError::Serialize {
            message: e.to_string(),
        })?;
        json.push('\n');
        std::fs::write(path, json).map_err(|e| ProjectSaveError::Io {
            error_kind: format!("{:?}", e.kind()),
            message: e.to_string(),
        })
    }

    /// Load a project from a single `.json` file at `path` (Spec §6, #38) — the
    /// native project-file reader. Reads the bytes (a read failure is
    /// [`ProjectLoadError::Io`]) then parses + version-validates them via
    /// [`from_json`] (so a malformed or out-of-date file is the corresponding typed
    /// [`ProjectLoadError`]). Never panics.
    ///
    /// [`from_json`]: Project::from_json
    pub fn load_from_file(path: impl AsRef<std::path::Path>) -> Result<Self, ProjectLoadError> {
        let json = std::fs::read_to_string(path).map_err(|e| ProjectLoadError::Io {
            error_kind: format!("{:?}", e.kind()),
            message: e.to_string(),
        })?;
        Self::from_json(&json)
    }
}

/// A typed error from saving a native project file ([`Project::save_to_file`] and
/// the `save_project` Tauri command, #38). Serializing a well-formed [`Project`]
/// effectively never fails, so in practice this is an [`Io`] write error; both
/// variants exist so the command never panics and the frontend can match.
///
/// [`Io`]: ProjectSaveError::Io
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum ProjectSaveError {
    /// The file could not be written (permission denied, missing parent dir, …).
    Io {
        /// The `std::io::ErrorKind` label (e.g. `"PermissionDenied"`). Named
        /// `errorKind` to avoid colliding with the `"kind"` serde tag.
        error_kind: String,
        /// The OS error message.
        message: String,
    },
    /// The project could not be serialized to JSON (should not happen for a
    /// well-formed model; present so the path never panics).
    Serialize {
        /// The underlying `serde_json` message.
        message: String,
    },
}

impl std::fmt::Display for ProjectSaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectSaveError::Io {
                error_kind,
                message,
            } => {
                write!(f, "could not write project file ({error_kind}): {message}")
            }
            ProjectSaveError::Serialize { message } => {
                write!(f, "could not serialize project: {message}")
            }
        }
    }
}

impl std::error::Error for ProjectSaveError {}

/// A typed error from loading a native project file ([`Project::from_json`] /
/// [`Project::load_from_file`], and the `load_project` Tauri command, #38).
/// Returned instead of panicking on a missing, malformed, or out-of-date file
/// (Spec §6 acceptance).
///
/// `std::io::Error` is not `Clone`/`Eq`/`Serialize`, so the [`Io`] variant
/// flattens a read failure to its `ErrorKind` label + message; this keeps the
/// whole enum a clean, serializable IPC payload the Phase-7 UX can match on (e.g.
/// distinguishing "file not found" from "corrupt file"). The content-only
/// helper [`Project::from_json`] never produces [`Io`].
///
/// [`Io`]: ProjectLoadError::Io
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum ProjectLoadError {
    /// The file could not be **read** from disk (missing, permission denied, …).
    /// Produced only by the file helper [`Project::load_from_file`], never by the
    /// in-memory [`Project::from_json`].
    Io {
        /// The `std::io::ErrorKind` label (e.g. `"NotFound"`), for matching.
        /// Named `errorKind` to avoid colliding with the `"kind"` serde tag.
        error_kind: String,
        /// The OS error message.
        message: String,
    },
    /// The text is not valid project JSON: not an object, missing
    /// `schemaVersion`, or a shape that doesn't match the current schema. Carries
    /// the underlying parser message.
    ///
    /// A **struct** variant (not a newtype) on purpose: serde cannot
    /// *internally-tag* a newtype variant whose payload is a plain string, so the
    /// message lives in a named `message` field — keeping this enum serializable
    /// as an IPC payload.
    Malformed {
        /// The underlying parser message.
        message: String,
    },
    /// The file's `schemaVersion` is **newer** than this build supports — it was
    /// written by a later version of the app. The user should update.
    TooNew {
        /// The version found in the file.
        found: u32,
        /// The newest version this build can read ([`PROJECT_SCHEMA_VERSION`]).
        supported: u32,
    },
    /// The file's `schemaVersion` is **older** than the current schema and no
    /// migration is registered for it yet. (When a breaking bump lands, its
    /// migration replaces this for the affected versions.)
    Unsupported {
        /// The version found in the file.
        found: u32,
        /// The current schema version ([`PROJECT_SCHEMA_VERSION`]).
        supported: u32,
    },
}

impl std::fmt::Display for ProjectLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectLoadError::Io {
                error_kind,
                message,
            } => {
                write!(f, "could not read project file ({error_kind}): {message}")
            }
            ProjectLoadError::Malformed { message } => {
                write!(f, "malformed project file: {message}")
            }
            ProjectLoadError::TooNew { found, supported } => write!(
                f,
                "project file schema version {found} is newer than this build supports \
                 (max {supported}); update ShaderBuilder to open it"
            ),
            ProjectLoadError::Unsupported { found, supported } => write!(
                f,
                "project file schema version {found} is no longer supported \
                 (current {supported}) and has no migration"
            ),
        }
    }
}

impl std::error::Error for ProjectLoadError {}

/// What the `export_preset` command wrote, returned to the webview (#36, Fix C1):
/// where the bundle landed and what files it contains, plus any non-fatal notes.
///
/// This is the success payload of the `export_preset` IPC command. It lives in
/// `core-model` — not in the `app` crate — so a TypeScript binding is generated
/// from the single shared schema (module doc §A) rather than escaping it as an
/// untyped, binding-less JSON shape. The richer [`preset_io::ExportReport`] is the
/// crate-internal writer report; this is its flattened, IPC-friendly projection
/// (absolute `presetPath` as a string, no `PathBuf`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportResult {
    /// Absolute path of the written `preset.slangp`.
    pub preset_path: String,
    /// Per-pass `.slang` file names written, relative to the bundle root.
    pub pass_files: Vec<String>,
    /// LUT file names written under `textures/`, relative to the bundle root.
    pub texture_files: Vec<String>,
    /// Non-fatal notes (e.g. a LUT source image that could not be copied in).
    pub warnings: Vec<String>,
}

/// A typed error from exporting a RetroArch `.slangp` bundle (the `export_preset`
/// command, #36, Fix C1). The webview-facing mirror of `preset_io::ExportError`:
/// it lives in `core-model` so a TypeScript binding is generated (module doc §A),
/// and — like [`ProjectSaveError`] / [`ProjectLoadError`] — it keeps the
/// semantically distinct failure modes as **branchable** variants instead of
/// collapsing them into one opaque string.
///
/// `std::io::Error` is not `Clone`/`Eq`/`Serialize`, so the [`Io`] variant carries
/// only a message string rather than leaking the OS error directly, keeping the
/// whole enum a clean, serializable IPC payload the Phase-7 export UX can match on.
///
/// [`Io`]: ExportError::Io
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum ExportError {
    /// The bundle could not be written (directory creation or file write failed).
    /// Carries a human-readable message (the flattened `std::io::Error`).
    Io {
        /// The underlying I/O error message.
        message: String,
    },
    /// A pass is authored as a node [`Graph`]; slang codegen for graph passes is a
    /// later phase, so it cannot be exported yet. This is an expected, user-facing
    /// limitation (distinct from an [`Io`] failure) — the frontend can surface it
    /// specifically. Carries the offending pass id.
    ///
    /// [`Io`]: ExportError::Io
    GraphPassUnsupported {
        /// The id of the graph pass that cannot be exported yet.
        pass_id: String,
    },
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::Io { message } => {
                write!(f, "could not write export bundle: {message}")
            }
            ExportError::GraphPassUnsupported { pass_id } => write!(
                f,
                "pass `{pass_id}` is a node graph; exporting graph passes to slang is not yet \
                 supported (whole-pass / imported passes only)"
            ),
        }
    }
}

impl std::error::Error for ExportError {}

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
            pipeline: PipelineMetadata {
                aliases: vec![AliasBinding {
                    alias: "FirstPass".to_owned(),
                    pass_index: 0,
                }],
                availability: vec![
                    PassAvailability {
                        pass_index: 0,
                        available: vec!["Original".to_owned(), "Source".to_owned()],
                    },
                    PassAvailability {
                        pass_index: 1,
                        available: vec![
                            "Original".to_owned(),
                            "Source".to_owned(),
                            "PassOutput0".to_owned(),
                            "FirstPass".to_owned(),
                        ],
                    },
                ],
            },
            parameters: vec![Parameter {
                name: "BRIGHTNESS".to_owned(),
                label: "Brightness".to_owned(),
                default: 1.5,
                min: 0.0,
                max: 2.0,
                step: 0.01,
            }],
            luts: vec![Lut {
                name: "BORDER".to_owned(),
                path: "luts/border.png".to_owned(),
                filter_linear: Some(true),
                wrap_mode: Some(WrapMode::ClampToEdge),
                mipmap: None,
            }],
            metadata: ProjectMetadata {
                description: Some("A demo project".to_owned()),
                author: Some("tester".to_owned()),
                created_at: Some("2026-06-17T00:00:00Z".to_owned()),
                modified_at: None,
            },
            library_refs: vec![LibraryRef {
                item_id: "lib-item-1".to_owned(),
                name: Some("Scanlines".to_owned()),
            }],
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
                    references: vec![],
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
                        filename: Some("imported.slang".to_owned()),
                        opaque: true,
                    },
                    parameters: vec![],
                    references: vec![TextureRef {
                        name: "Source".to_owned(),
                        kind: TextureRefKind::Source,
                    }],
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
            filename: Some("x.slang".to_owned()),
            opaque: true,
        };
        let value = serde_json::to_value(&code).unwrap();
        assert_eq!(value["kind"], "wholePassCode");
        assert_eq!(value["source"], "x");
        assert_eq!(value["filename"], "x.slang");
        assert_eq!(value["opaque"], true);
    }

    #[test]
    fn whole_pass_code_filename_and_opaque_default() {
        // Older project files (no `filename`/`opaque`) still load: filename
        // defaults to None and the source is treated as opaque (non-decomposable).
        let src: PassSource =
            serde_json::from_str(r#"{"kind":"wholePassCode","source":"// body"}"#)
                .expect("legacy wholePassCode deserializes");
        match src {
            PassSource::WholePassCode {
                source,
                filename,
                opaque,
            } => {
                assert_eq!(source, "// body");
                assert_eq!(filename, None);
                assert!(
                    opaque,
                    "whole-pass code defaults to opaque/non-decomposable"
                );
            }
            other => panic!("expected whole-pass code, got {other:?}"),
        }
    }

    #[test]
    fn pipeline_metadata_defaults_to_empty() {
        // An omitted `pipeline`/`references` deserializes to defaults so older
        // project files (schemaVersion 1, no pipeline metadata) still load.
        let project: Project = serde_json::from_str(
            r#"{"schemaVersion":1,"name":"x","passes":[{"id":"p","name":"n","source":{"kind":"wholePassCode","source":""},"parameters":[]}]}"#,
        )
        .expect("project without pipeline deserializes");
        assert_eq!(project.pipeline, PipelineMetadata::default());
        assert!(project.passes[0].references.is_empty());
    }

    #[test]
    fn project_parameters_and_luts_default_to_empty() {
        // An omitted `parameters`/`luts` on the project deserializes to `[]` so
        // older project files (schemaVersion 1, pre-#35) still load.
        let project: Project =
            serde_json::from_str(r#"{"schemaVersion":1,"name":"x","passes":[]}"#)
                .expect("project without parameters/luts deserializes");
        assert!(project.parameters.is_empty());
        assert!(project.luts.is_empty());
        assert_eq!(Project::empty("x").parameters, Vec::<Parameter>::new());
        assert_eq!(Project::empty("x").luts, Vec::<Lut>::new());
    }

    #[test]
    fn lut_optional_sampler_keys_default_to_none() {
        // A LUT with only name+path (no `_linear`/`_wrap_mode`/`_mipmap`) loads
        // with all sampler settings `None` so the engine applies §7 defaults.
        let lut: Lut = serde_json::from_str(r#"{"name":"BORDER","path":"luts/border.png"}"#)
            .expect("minimal LUT deserializes");
        assert_eq!(lut.name, "BORDER");
        assert_eq!(lut.path, "luts/border.png");
        assert_eq!(lut.filter_linear, None);
        assert_eq!(lut.wrap_mode, None);
        assert_eq!(lut.mipmap, None);
    }

    #[test]
    fn metadata_and_library_refs_default_to_empty() {
        // An omitted `metadata`/`libraryRefs` deserializes to defaults so older
        // project files (pre-#38) still load.
        let project: Project =
            serde_json::from_str(r#"{"schemaVersion":1,"name":"x","passes":[]}"#)
                .expect("project without metadata/libraryRefs deserializes");
        assert_eq!(project.metadata, ProjectMetadata::default());
        assert!(project.library_refs.is_empty());
        assert_eq!(Project::empty("x").metadata, ProjectMetadata::default());
        assert_eq!(Project::empty("x").library_refs, Vec::<LibraryRef>::new());
    }

    #[test]
    fn project_metadata_serializes_camel_case() {
        let meta = ProjectMetadata {
            description: Some("d".to_owned()),
            author: Some("a".to_owned()),
            created_at: Some("2026-06-17T00:00:00Z".to_owned()),
            modified_at: Some("2026-06-18T00:00:00Z".to_owned()),
        };
        let json = serde_json::to_value(&meta).unwrap();
        assert_eq!(json["createdAt"], "2026-06-17T00:00:00Z");
        assert_eq!(json["modifiedAt"], "2026-06-18T00:00:00Z");
        let back: ProjectMetadata = serde_json::from_value(json).unwrap();
        assert_eq!(meta, back);
    }

    #[test]
    fn library_ref_round_trips() {
        let r = LibraryRef {
            item_id: "abc".to_owned(),
            name: Some("Scanlines".to_owned()),
        };
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["itemId"], "abc");
        let back: LibraryRef = serde_json::from_value(json).unwrap();
        assert_eq!(r, back);
        // `name` is optional.
        let minimal: LibraryRef = serde_json::from_str(r#"{"itemId":"x"}"#).unwrap();
        assert_eq!(minimal.name, None);
    }

    #[test]
    fn to_from_json_round_trips_a_multipass_project() {
        // The native-project-file (#38) round trip: a project with multiple
        // passes, a whole-pass code node, parameters, metadata, and library refs
        // survives to_json -> from_json identically.
        let project = Project {
            schema_version: PROJECT_SCHEMA_VERSION,
            name: "Multipass".to_owned(),
            feedback_pass: Some(0),
            pipeline: PipelineMetadata::default(),
            parameters: vec![Parameter {
                name: "GAIN".to_owned(),
                label: "Gain".to_owned(),
                default: 1.0,
                min: 0.0,
                max: 4.0,
                step: 0.1,
            }],
            luts: vec![],
            metadata: ProjectMetadata {
                description: Some("round trip".to_owned()),
                author: None,
                created_at: Some("2026-06-17T00:00:00Z".to_owned()),
                modified_at: None,
            },
            library_refs: vec![LibraryRef {
                item_id: "lib-1".to_owned(),
                name: None,
            }],
            passes: vec![
                Pass {
                    id: "p0".to_owned(),
                    name: "First".to_owned(),
                    source: PassSource::WholePassCode {
                        source: "// pass 0\r\nvoid main() {}\n".to_owned(),
                        filename: Some("p0.slang".to_owned()),
                        opaque: true,
                    },
                    parameters: vec![],
                    references: vec![],
                    settings: PassSettings::default(),
                },
                Pass {
                    id: "p1".to_owned(),
                    name: "Second".to_owned(),
                    source: PassSource::Graph {
                        graph: Graph::default(),
                    },
                    parameters: vec![Parameter {
                        name: "GAIN".to_owned(),
                        label: "Gain".to_owned(),
                        default: 1.0,
                        min: 0.0,
                        max: 4.0,
                        step: 0.1,
                    }],
                    references: vec![],
                    settings: PassSettings::default(),
                },
            ],
        };

        let json = project.to_json().expect("serialize");
        let back = Project::from_json(&json).expect("deserialize");
        assert_eq!(project, back);
    }

    #[test]
    fn save_to_file_then_load_from_file_round_trips() {
        // The on-disk #38 round trip: save a project to one .json, load it back,
        // and get an identical in-memory model.
        let project = Project {
            schema_version: PROJECT_SCHEMA_VERSION,
            name: "OnDisk".to_owned(),
            metadata: ProjectMetadata {
                description: Some("disk round trip".to_owned()),
                ..ProjectMetadata::default()
            },
            ..Project::empty("OnDisk")
        };

        let path = std::env::temp_dir().join(format!(
            "sb-project-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        project.save_to_file(&path).expect("save");
        let loaded = Project::load_from_file(&path).expect("load");
        assert_eq!(project, loaded);
        // The written file ends in a newline (tool-friendly) but still parses.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.ends_with('\n'));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_from_file_missing_file_is_typed_io_error() {
        let path = std::env::temp_dir().join(format!(
            "sb-project-does-not-exist-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        match Project::load_from_file(&path) {
            Err(ProjectLoadError::Io { error_kind, .. }) => assert_eq!(error_kind, "NotFound"),
            other => panic!("expected Io(NotFound), got {other:?}"),
        }
    }

    #[test]
    fn project_save_error_serializes_as_ipc_payload() {
        for err in [
            ProjectSaveError::Io {
                error_kind: "PermissionDenied".to_owned(),
                message: "denied".to_owned(),
            },
            ProjectSaveError::Serialize {
                message: "bad".to_owned(),
            },
        ] {
            let value = serde_json::to_value(&err).expect("serializes");
            let back: ProjectSaveError = serde_json::from_value(value).expect("round-trips");
            assert_eq!(err, back);
        }
    }

    #[test]
    fn export_error_serializes_as_branchable_ipc_payload() {
        // The webview-facing export error must round-trip through serde and keep
        // its variants distinguishable by the "kind" tag (Fix C1) — so the
        // frontend can branch on GraphPassUnsupported specifically.
        for err in [
            ExportError::Io {
                message: "disk full".to_owned(),
            },
            ExportError::GraphPassUnsupported {
                pass_id: "pass-0".to_owned(),
            },
        ] {
            let value = serde_json::to_value(&err).expect("serializes");
            assert!(
                value.get("kind").and_then(|k| k.as_str()).is_some(),
                "export error carries a `kind` discriminator: {value}"
            );
            let back: ExportError = serde_json::from_value(value).expect("round-trips");
            assert_eq!(err, back);
        }
        // The GraphPassUnsupported case is branchable on its typed field.
        let graph = ExportError::GraphPassUnsupported {
            pass_id: "p1".to_owned(),
        };
        match graph {
            ExportError::GraphPassUnsupported { pass_id } => assert_eq!(pass_id, "p1"),
            other => panic!("expected GraphPassUnsupported, got {other:?}"),
        }
    }

    #[test]
    fn export_result_round_trips_camel_case() {
        let result = ExportResult {
            preset_path: "/tmp/out/preset.slangp".to_owned(),
            pass_files: vec!["a.slang".to_owned()],
            texture_files: vec!["border.png".to_owned()],
            warnings: vec!["note".to_owned()],
        };
        let value = serde_json::to_value(&result).expect("serializes");
        assert!(
            value.get("presetPath").is_some(),
            "fields are camelCase: {value}"
        );
        let back: ExportResult = serde_json::from_value(value).expect("round-trips");
        assert_eq!(result, back);
    }

    #[test]
    fn from_json_rejects_a_newer_schema_version() {
        let json = format!(
            r#"{{"schemaVersion":{},"name":"x","passes":[]}}"#,
            PROJECT_SCHEMA_VERSION + 1
        );
        match Project::from_json(&json) {
            Err(ProjectLoadError::TooNew { found, supported }) => {
                assert_eq!(found, PROJECT_SCHEMA_VERSION + 1);
                assert_eq!(supported, PROJECT_SCHEMA_VERSION);
            }
            other => panic!("expected TooNew, got {other:?}"),
        }
    }

    #[test]
    fn from_json_rejects_an_older_unsupported_schema_version() {
        // v1 is the floor today, so v0 has no migration registered.
        let json = r#"{"schemaVersion":0,"name":"x","passes":[]}"#;
        match Project::from_json(json) {
            Err(ProjectLoadError::Unsupported { found, supported }) => {
                assert_eq!(found, 0);
                assert_eq!(supported, PROJECT_SCHEMA_VERSION);
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn from_json_reports_malformed_input_without_panicking() {
        // Not JSON at all.
        assert!(matches!(
            Project::from_json("not json"),
            Err(ProjectLoadError::Malformed { .. })
        ));
        // Valid JSON but missing schemaVersion.
        assert!(matches!(
            Project::from_json(r#"{"name":"x","passes":[]}"#),
            Err(ProjectLoadError::Malformed { .. })
        ));
        // Right version but a required field (`name`) is absent.
        let json = format!(
            r#"{{"schemaVersion":{},"passes":[]}}"#,
            PROJECT_SCHEMA_VERSION
        );
        assert!(matches!(
            Project::from_json(&json),
            Err(ProjectLoadError::Malformed { .. })
        ));
    }

    #[test]
    fn project_load_error_serializes_as_ipc_payload() {
        // The error type rides the IPC channel, so every variant must serialize
        // (a tagged *newtype* string variant would fail at runtime — see the
        // `message` struct field on `Malformed`).
        for err in [
            ProjectLoadError::Io {
                error_kind: "NotFound".to_owned(),
                message: "missing".to_owned(),
            },
            ProjectLoadError::Malformed {
                message: "boom".to_owned(),
            },
            ProjectLoadError::TooNew {
                found: 2,
                supported: 1,
            },
            ProjectLoadError::Unsupported {
                found: 0,
                supported: 1,
            },
        ] {
            let value = serde_json::to_value(&err).expect("error serializes");
            let back: ProjectLoadError = serde_json::from_value(value).expect("error round-trips");
            assert_eq!(err, back);
        }
        // The discriminator is `kind` and `Malformed` exposes a named `message`.
        let v = serde_json::to_value(ProjectLoadError::Malformed {
            message: "m".to_owned(),
        })
        .unwrap();
        assert_eq!(v["kind"], "malformed");
        assert_eq!(v["message"], "m");
    }

    #[test]
    fn lut_round_trips_with_sampler_settings() {
        let lut = Lut {
            name: "OVERLAY".to_owned(),
            path: "../shared/overlay.png".to_owned(),
            filter_linear: Some(false),
            wrap_mode: Some(WrapMode::Repeat),
            mipmap: Some(true),
        };
        let json = serde_json::to_value(&lut).unwrap();
        // camelCase field names per the §A convention.
        assert_eq!(json["filterLinear"], serde_json::json!(false));
        assert_eq!(json["wrapMode"], serde_json::json!("repeat"));
        assert_eq!(json["mipmap"], serde_json::json!(true));
        let back: Lut = serde_json::from_value(json).unwrap();
        assert_eq!(lut, back);
    }
}
