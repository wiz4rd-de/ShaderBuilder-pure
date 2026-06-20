//! Builtin + parameter uniform computation for a single preview pass
//! (Architecture §D, `docs/retroarch-slang-runtime.md` §6).
//!
//! ## Reflection-driven packing (#28)
//!
//! Real RetroArch shaders declare the builtin block members in arbitrary order
//! and as arbitrary subsets, so the byte offsets cannot be assumed. The renderer
//! reflects each pass's SPIR-V (`slang_compile::reflect`) to discover the
//! builtin block's member names → offsets, then [`pack_builtins`] writes each
//! [`BuiltinValues`] semantic at the offset of the same-named member. The member
//! *order* in the shader is irrelevant; a member matching no known semantic
//! (e.g. a `#pragma parameter` in a shared block — #29 — or a not-yet-wired
//! resource size) is left at its zero initialization. This is the same
//! offset-by-name mechanism #29 will reuse for live parameter values.
//!
//! Each `*Size` vec4 is `[w, h, 1/w, 1/h]` — the RetroArch convention that lets a
//! shader fetch both a dimension and its reciprocal without a divide.
//!
//! ## Legacy fixed-layout struct
//!
//! [`BuiltinUniforms`] is the Phase 1 fixed-layout `#[repr(C)]` mirror of the
//! canonical block (`MVP`, `SourceSize`, `OriginalSize`, `OutputSize`,
//! `FrameCount`). It is retained for unit testing the std140 offsets the
//! reflection path must reproduce; the renderer no longer writes it directly.

use slang_compile::{Parameter, SpirvReflection, UniformBlock};

/// The builtin uniforms for one pass, laid out to match the canonical std140 UBO
/// block (see the module docs). `#[repr(C)]` + field order/padding make the byte
/// image bindable directly via `bytemuck`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BuiltinUniforms {
    /// Model-view-projection matrix, column-major (offset 0).
    pub mvp: [f32; 16],
    /// `Source` texture size `[w, h, 1/w, 1/h]` (offset 64).
    pub source_size: [f32; 4],
    /// `Original` (pass-0 input) size; equals `source_size` in a one-pass slice
    /// (offset 80).
    pub original_size: [f32; 4],
    /// Render-target / simulated-viewport size (offset 96).
    pub output_size: [f32; 4],
    /// Frames rendered so far, for animated shaders (offset 112).
    pub frame_count: u32,
    /// Tail padding so the struct is a multiple of 16 bytes, as std140 requires
    /// for a UBO block. Always zero.
    pub pad: [u32; 3],
}

impl BuiltinUniforms {
    /// Compute the builtin set from the source-image and output (viewport) sizes
    /// and the current frame counter. `original` is the pass-0 input — in a
    /// one-pass slice it is the same image as `source`. Convenience wrapper over
    /// [`BuiltinUniforms::new_full`] for the single-pass case where
    /// `Source == Original`.
    pub fn new(source: (u32, u32), output: (u32, u32), frame_count: u32) -> Self {
        Self::new_full(source, source, output, frame_count)
    }

    /// Compute the builtin set for one pass of a multi-pass chain, where the
    /// pass's `Source` (its input), the chain's `Original` (the pass-0 input),
    /// and the pass's `Output` (its render target) can all differ (§2/§6). Pass 0
    /// has `source == original`; later passes' `source` is the previous FBO size.
    pub fn new_full(
        source: (u32, u32),
        original: (u32, u32),
        output: (u32, u32),
        frame_count: u32,
    ) -> Self {
        Self {
            mvp: ortho_mvp(),
            source_size: size_vec(source.0, source.1),
            original_size: size_vec(original.0, original.1),
            output_size: size_vec(output.0, output.1),
            frame_count,
            pad: [0; 3],
        }
    }

    /// The struct as raw bytes ready to upload into the builtin UBO.
    pub fn as_bytes(&self) -> &[u8] {
        bytemuck::bytes_of(self)
    }
}

/// A RetroArch `*Size` vector: `[w, h, 1/w, 1/h]`. Dimensions are clamped to at
/// least 1 so the reciprocals are always finite.
pub fn size_vec(width: u32, height: u32) -> [f32; 4] {
    let w = width.max(1) as f32;
    let h = height.max(1) as f32;
    [w, h, 1.0 / w, 1.0 / h]
}

/// The fullscreen-pass MVP: an orthographic projection mapping the unit-square
/// quad (positions in `[0,1]`) to clip space `[-1,1]`. This is RetroArch's
/// standard pass matrix and, like RetroArch's, it does not depend on the
/// viewport size — the viewport governs rasterization, not this transform.
/// Returned column-major to match GLSL/SPIR-V `mat4` storage.
pub fn ortho_mvp() -> [f32; 16] {
    [
        2.0, 0.0, 0.0, 0.0, // column 0
        0.0, 2.0, 0.0, 0.0, // column 1
        0.0, 0.0, 1.0, 0.0, // column 2
        -1.0, -1.0, 0.0, 1.0, // column 3 (translation)
    ]
}

/// Pack the parameter defaults into the std140 parameter-UBO byte image: each
/// `#pragma parameter` default as a consecutive `f32` (4-byte std140 scalar
/// packing), in declaration order, then padded up to a multiple of 16 bytes
/// (never empty — a bound UBO needs at least one vec4 of storage even when the
/// shader declares no parameters). Live updates are Phase 2; this writes the
/// reflected defaults only.
pub fn pack_parameters(params: &[Parameter]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(params.len() * 4);
    for p in params {
        bytes.extend_from_slice(&p.default.to_le_bytes());
    }
    let padded = bytes.len().div_ceil(16).max(1) * 16;
    bytes.resize(padded, 0);
    bytes
}

// ============================================================================
// Reflection-driven parameter packing + global parameter state (#29)
// ============================================================================

/// The live, global-by-name parameter state for a render chain (#29).
///
/// RetroArch parameters are **global by name**: a `#pragma parameter X` declared
/// in any number of passes is one knob with one `current` value, fed to every
/// block (UBO or push) that declares a member named `X`. This store holds the
/// deduped parameter metadata (`#pragma parameter` defaults/range/label, plus any
/// alias) in declaration order, and a `name -> current value` map the renderer
/// re-packs into each pass's reflected offsets every frame.
///
/// Values are stored **raw** in `current` (no clamp on set) and clamped to
/// `[min, max]` only at **use** time, when [`pack_params`] writes a value into a
/// UBO (the reference's §11 item 7: RetroArch stores the raw float into
/// `current`; the clamp is applied where the value is consumed). Construction
/// seeds each `current` to the `#pragma` default; [`ParamStore::apply_overrides`]
/// then layers a preset's `parameter_overrides` on top (the §8 `id = value`
/// semantics: overrides the `current` value, not the pragma `initial`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParamStore {
    /// Deduped parameter metadata in first-seen declaration order. The slider UI
    /// renders these; the renderer reads `name`/`alias` to know which member
    /// names a value packs into.
    params: Vec<ParamDef>,
    /// Current value per canonical name (initialized to each default, then
    /// overridden). Looked up by both the canonical name and any alias.
    current: std::collections::HashMap<String, f32>,
}

/// One global parameter's metadata: the `#pragma parameter` declaration plus an
/// optional alternate name (`alias`) the slang/preset may reference it by (§8).
/// A value set via the alias drives the same `current` slot as the canonical
/// name.
#[derive(Clone, Debug, PartialEq)]
pub struct ParamDef {
    /// The canonical `#pragma parameter` identifier (the UBO member name).
    pub name: String,
    /// An alternate name this parameter is also addressable by, if any. Both the
    /// canonical name and the alias resolve to the same `current` value.
    pub alias: Option<String>,
    /// Human-readable label (`#pragma parameter` description).
    pub label: String,
    /// `#pragma` default (the `initial` field; never mutated by a set/override).
    pub default: f32,
    /// Minimum (clamp lower bound).
    pub min: f32,
    /// Maximum (clamp upper bound).
    pub max: f32,
    /// Slider increment (informational; surfaced to the UI).
    pub step: f32,
}

/// A current parameter value surfaced to the UI / a query: the metadata plus the
/// live `value`. Returned by the engine's `parameters()` query so a frontend can
/// render data-driven sliders.
#[derive(Clone, Debug, PartialEq)]
pub struct ParamView {
    /// Canonical `#pragma parameter` name.
    pub name: String,
    /// Human-readable label.
    pub label: String,
    /// Minimum slider value.
    pub min: f32,
    /// Maximum slider value.
    pub max: f32,
    /// Slider increment.
    pub step: f32,
    /// The current (live) value.
    pub value: f32,
}

impl ParamStore {
    /// Build a store from every pass's `#pragma parameter` declarations, deduped
    /// **by exact name** (RetroArch's global-by-name merge, §8): the first
    /// declaration of a name wins for metadata and the rest are merged into it.
    /// Each `current` value is seeded to the parameter's default. `aliases` maps a
    /// canonical parameter name to an alternate name it may also be referenced by
    /// (e.g. a preset/pass alias); an empty map means no aliases.
    ///
    /// Per §8 the same id declared across files must match exactly; we do not hard
    /// error on a mismatch here (the compile/preprocess layer is the place for
    /// that) — we keep the first declaration, which is the value RetroArch uses.
    pub fn collect<'a>(
        passes: impl IntoIterator<Item = &'a [Parameter]>,
        aliases: &std::collections::HashMap<String, String>,
    ) -> Self {
        let mut store = ParamStore::default();
        for params in passes {
            for p in params {
                if store.params.iter().any(|d| d.name == p.name) {
                    continue; // already collected (global by name)
                }
                let alias = aliases.get(&p.name).cloned();
                store.current.insert(p.name.clone(), p.default);
                if let Some(a) = &alias {
                    store.current.insert(a.clone(), p.default);
                }
                store.params.push(ParamDef {
                    name: p.name.clone(),
                    alias,
                    label: p.label.clone(),
                    default: p.default,
                    min: p.min,
                    max: p.max,
                    step: p.step,
                });
            }
        }
        store
    }

    /// Apply a preset's `parameter_overrides` (§8 bare `id = value`): set each
    /// named parameter's current value (stored raw; §11 item 7). Names that match
    /// no collected parameter are ignored. An override may target the canonical
    /// name or an alias.
    pub fn apply_overrides(&mut self, overrides: &std::collections::BTreeMap<String, f32>) {
        for (name, value) in overrides {
            self.set(name, *value);
        }
    }

    /// Set a parameter's current value by canonical name or alias. The **raw**
    /// value is stored (§11 item 7: RetroArch stores the raw float into `current`;
    /// the `[min, max]` clamp is applied at use time, in [`pack_params`], so the
    /// rendered pixel stays clamped while the surfaced value is the raw input). A
    /// name matching no parameter is a no-op (returns `false`); a successful set
    /// returns `true`.
    pub fn set(&mut self, name: &str, value: f32) -> bool {
        // Resolve the name (or alias) to the canonical parameter.
        let Some(def) = self
            .params
            .iter()
            .find(|d| d.name == name || d.alias.as_deref() == Some(name))
        else {
            return false;
        };
        let (canonical, alias) = (def.name.clone(), def.alias.clone());
        // Store the RAW value under the canonical name and the alias so a member
        // declared under either spelling reads the same value.
        self.current.insert(canonical, value);
        if let Some(a) = alias {
            self.current.insert(a, value);
        }
        true
    }

    /// The current **raw** value for a member `name` (canonical or alias), or
    /// `None` if no parameter is named that. This is the unclamped value the user
    /// set (§11 item 7); the clamp to `[min, max]` is applied at packing time by
    /// [`ParamStore::clamped_value`] / [`pack_params`].
    pub fn value(&self, name: &str) -> Option<f32> {
        self.current.get(name).copied()
    }

    /// The current value for a member `name` (canonical or alias) **clamped** to
    /// its `[min, max]` range — what [`pack_params`] writes into a UBO so the
    /// rendered pixel stays in range (§11 item 7). `None` if no parameter is named
    /// that. The clamp uses the member's own [`ParamDef`] (looked up by name or
    /// alias); a value with no matching def is returned unclamped.
    pub fn clamped_value(&self, name: &str) -> Option<f32> {
        let raw = self.current.get(name).copied()?;
        let def = self
            .params
            .iter()
            .find(|d| d.name == name || d.alias.as_deref() == Some(name));
        Some(match def {
            Some(d) => raw.clamp(d.min, d.max),
            None => raw,
        })
    }

    /// Whether the store holds no parameters at all.
    pub fn is_empty(&self) -> bool {
        self.params.is_empty()
    }

    /// The current parameter set for the UI / a query, in declaration order.
    pub fn views(&self) -> Vec<ParamView> {
        self.params
            .iter()
            .map(|d| ParamView {
                name: d.name.clone(),
                label: d.label.clone(),
                min: d.min,
                max: d.max,
                step: d.step,
                value: self.value(&d.name).unwrap_or(d.default),
            })
            .collect()
    }
}

/// Overlay the current parameter values onto an already-packed block byte image
/// (#29): for every member of `block` whose name matches a parameter in `store`,
/// write that parameter's current value (an `f32`) at the member's reflected
/// offset. Members that are builtins or unknown are left untouched — so this is
/// safe to run after [`pack_builtins`] on the *same* block, giving one
/// "builtins first, then params" path for a mixed block (as real RetroArch
/// shaders declare). Mutates `bytes` in place; a no-op when the store is empty.
///
/// This is the same offset-by-name mechanism #28 uses for builtins, layered for
/// parameters: a member is a builtin XOR a parameter. A member whose name is a
/// builtin semantic is **skipped** here even if a `#pragma parameter` happens to
/// collide with it — the builtin wins, matching RetroArch (#28/#29). The value
/// written is **clamped** to the parameter's `[min, max]` range (§11 item 7: the
/// store keeps the raw value; the clamp is applied at this point of use so the
/// rendered pixel stays in range).
pub fn pack_params(bytes: &mut [u8], block: &UniformBlock, store: &ParamStore) {
    if store.is_empty() {
        return;
    }
    for m in &block.members {
        // A name collision between a `#pragma parameter` and a builtin semantic
        // resolves in the builtin's favor (it was already packed by
        // `pack_builtins`); never let the parameter overwrite it (#28/#29).
        if is_builtin_semantic(&m.name) {
            continue;
        }
        let Some(value) = store.clamped_value(&m.name) else {
            continue;
        };
        let src = value.to_le_bytes();
        let start = m.offset as usize;
        let max = (m.size as usize).min(src.len());
        let end = (start + max).min(bytes.len());
        if start < bytes.len() {
            bytes[start..end].copy_from_slice(&src[..end - start]);
        }
    }
}

// ============================================================================
// Reflection-driven builtin semantics (#28)
// ============================================================================

/// Every currently-computable RetroArch builtin semantic value for one pass
/// (`docs/retroarch-slang-runtime.md` §6). A shader declares whichever of these
/// it wants, in any order or subset, inside its builtin UBO/push block; the
/// renderer reflects that block's member offsets (#28 infra in `slang-compile`)
/// and [`pack_builtins`] writes each value at the offset of the member whose
/// **name matches the semantic**. This is the same offset-by-name mechanism #29
/// will use for `#pragma parameter` values.
///
/// Semantics whose backing resource doesn't exist yet (`PassFeedbackNSize`,
/// `OriginalHistoryNSize`, LUT `<NAME>Size`) are simply absent here: a member
/// referencing one is left untouched (zero-initialized), which is the graceful
/// "write zeros / skip" the spec asks for — those land in #24/#25/#27.
#[derive(Clone, Debug, PartialEq)]
pub struct BuiltinValues {
    /// `MVP` — the fullscreen-pass orthographic matrix (column-major mat4).
    pub mvp: [f32; 16],
    /// `SourceSize` — this pass's input size, `[w,h,1/w,1/h]`.
    pub source_size: [f32; 4],
    /// `OriginalSize` — the chain's pass-0 input size.
    pub original_size: [f32; 4],
    /// `OutputSize` — this pass's render-target size.
    pub output_size: [f32; 4],
    /// `FinalViewportSize` — the simulated final viewport / pane size.
    pub final_viewport_size: [f32; 4],
    /// `FrameCount` (uint) — already wrapped by this pass's `frame_count_mod`.
    pub frame_count: u32,
    /// `FrameDirection` (int) — `+1` forward, `-1` rewinding (#31).
    pub frame_direction: i32,
    /// `Rotation` (uint) — content rotation 0..3 (0 for now).
    pub rotation: u32,
    /// Earlier passes' output sizes, indexed by pass number. `pass_output_sizes[k]`
    /// is pass `k`'s output `[w,h,1/w,1/h]`, available to pass `i` for `k < i`
    /// (causal — §7). Backs both spellings `PassOutputKSize` and `PassKSize`.
    pub pass_output_sizes: Vec<[f32; 4]>,
    /// Aliased passes' output sizes by alias name (#26): `alias_sizes["FOO"]` is
    /// the `[w,h,1/w,1/h]` of the pass whose preset `aliasN == FOO`, available to
    /// a later pass as the `FOOSize` builtin (§6/§7). Only causally-available
    /// aliases are present; an absent alias leaves `<alias>Size` zero.
    pub alias_sizes: std::collections::HashMap<String, [f32; 4]>,
    /// Every feedback target's previous-frame output size (#24, §4), indexed by
    /// pass number: `pass_feedback_sizes[k]` backs `PassFeedbackKSize`. The
    /// feedback twin has the same dimensions as the pass's output, and feedback is
    /// time-causal, so — unlike `pass_output_sizes` — this is **not** restricted to
    /// earlier passes: any `k` may be read. An absent index leaves the member zero.
    pub pass_feedback_sizes: Vec<[f32; 4]>,
    /// Feedback sizes by alias name (#24, §4): `alias_feedback_sizes["FOO"]` backs
    /// the `FOOFeedbackSize` builtin — the previous-frame output size of the pass
    /// aliased `FOO`. Same dimensions as `alias_sizes["FOO"]`, but populated for
    /// all aliased feedback targets regardless of causal order.
    pub alias_feedback_sizes: std::collections::HashMap<String, [f32; 4]>,
    /// History-frame sizes (#25, §5): `original_history_sizes[k-1]` backs
    /// `OriginalHistoryKSize` (K≥1). A cold slot reports the current source size
    /// (the ring is pre-allocated to the input size), so reciprocals are safe.
    /// `OriginalHistory0Size` ≡ `OriginalSize` and is served from `original_size`.
    pub original_history_sizes: Vec<[f32; 4]>,
    /// LUT sizes by name (#27, §7): `lut_sizes["NAME"]` backs the `<NAME>Size`
    /// builtin — the LUT's pixel dimensions. Absent for an unregistered name.
    pub lut_sizes: std::collections::HashMap<String, [f32; 4]>,
}

impl Default for BuiltinValues {
    fn default() -> Self {
        Self {
            mvp: ortho_mvp(),
            source_size: [0.0; 4],
            original_size: [0.0; 4],
            output_size: [0.0; 4],
            final_viewport_size: [0.0; 4],
            frame_count: 0,
            frame_direction: 1,
            rotation: 0,
            pass_output_sizes: Vec::new(),
            alias_sizes: std::collections::HashMap::new(),
            pass_feedback_sizes: Vec::new(),
            alias_feedback_sizes: std::collections::HashMap::new(),
            original_history_sizes: Vec::new(),
            lut_sizes: std::collections::HashMap::new(),
        }
    }
}

impl BuiltinValues {
    /// Resolve a builtin member `name` to its packed little-endian bytes, or
    /// `None` if `name` is not a semantic this engine computes (the member is
    /// then left untouched — e.g. a `#pragma parameter`, handled by #29, or a
    /// not-yet-wired resource size).
    ///
    /// Both `PassOutputKSize` and the `PassKSize` alias resolve to the same
    /// pass-`K` output size (§7 / §11 ⚠ — both spellings are accepted). An index
    /// past the known passes (a not-yet-run later pass, or feedback/history)
    /// returns `None` so the member stays zero.
    pub fn member_bytes(&self, name: &str) -> Option<Vec<u8>> {
        let vec4 = |v: &[f32; 4]| Some(v.iter().flat_map(|f| f.to_le_bytes()).collect());
        match name {
            "MVP" => Some(self.mvp.iter().flat_map(|f| f.to_le_bytes()).collect()),
            "SourceSize" => vec4(&self.source_size),
            "OriginalSize" => vec4(&self.original_size),
            "OutputSize" => vec4(&self.output_size),
            "FinalViewportSize" => vec4(&self.final_viewport_size),
            "FrameCount" => Some(self.frame_count.to_le_bytes().to_vec()),
            "FrameDirection" => Some(self.frame_direction.to_le_bytes().to_vec()),
            "Rotation" => Some(self.rotation.to_le_bytes().to_vec()),
            _ => {
                // `<alias>Size` (#26): the aliased pass's output size, if that
                // alias is causally available this pass. Checked before the
                // PassK fallback so an alias spelled like `Pass…` can't be
                // misread (aliases are author-chosen identifiers).
                if let Some(base) = name.strip_suffix("Size") {
                    // `<alias>FeedbackSize` (#24) is matched before the plain
                    // `<alias>Size`: strip the `Feedback` suffix and look the alias
                    // up in the feedback-size table (same dims as its output, but
                    // valid for any causal-in-time feedback read).
                    if let Some(alias) = base.strip_suffix("Feedback") {
                        if let Some(v) = self.alias_feedback_sizes.get(alias) {
                            return vec4(v);
                        }
                    }
                    if let Some(v) = self.alias_sizes.get(base) {
                        return vec4(v);
                    }
                    // `<NAME>Size` for a registered LUT (#27, §7).
                    if let Some(v) = self.lut_sizes.get(base) {
                        return vec4(v);
                    }
                }
                // `PassFeedbackKSize` (#24) — pass K's previous-frame output size;
                // time-causal, so any K is valid.
                if let Some(idx) = parse_pass_feedback_size_index(name) {
                    return self.pass_feedback_sizes.get(idx).and_then(vec4);
                }
                // `OriginalHistoryKSize` (#25, §5) — the size of the source frame K
                // frames ago. `OriginalHistory0Size` ≡ `OriginalSize`; `K≥1` reads
                // ring slot `K-1`.
                if let Some(k) = parse_history_size_index(name) {
                    return match k {
                        0 => vec4(&self.original_size),
                        _ => self.original_history_sizes.get(k - 1).and_then(vec4),
                    };
                }
                // `PassOutputKSize` (and the `PassKSize` alias) — pass K's output
                // size, K < this pass (causal). Anything else is unknown here.
                let idx = parse_pass_size_index(name)?;
                self.pass_output_sizes.get(idx).and_then(vec4)
            }
        }
    }
}

/// Parse the index out of an indexed-size semantic for a given `base`, accepting
/// **both** spellings RetroArch's `slang_process.cpp` produces and accepts:
///
/// * the canonical *uniform* spelling `<base>Size<N>` — what RetroArch actually
///   emits (it builds the size-member name as `names[semantic]` (e.g.
///   `"PassFeedbackSize"`, `"PassOutputSize"`, `"OriginalHistorySize"`) **then**
///   appends the index), so the real corpus uses `PassFeedbackSize0`,
///   `PassOutputSize1`, `OriginalHistorySize2`, … (Size BEFORE the number); and
/// * the *alias* spelling `<base><N>Size` (Size AFTER the number) — the form a
///   `#pragma name`/alias produces (`name + "Size"`) and the spelling earlier
///   tickets assumed.
///
/// Returns the parsed `N` for either spelling, `None` otherwise. The `<base>Size<N>`
/// branch is tried first because for `base = "Pass"` the alias branch would
/// otherwise mis-read `PassFeedbackSize0` / `PassOutputSize0` (its remainder is
/// non-numeric, so it would still reject — but ordering keeps intent clear).
fn parse_indexed_size(name: &str, base: &str) -> Option<usize> {
    // Canonical RetroArch uniform spelling: `<base>Size<N>`.
    if let Some(rest) = name.strip_prefix(base).and_then(|r| r.strip_prefix("Size")) {
        if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()) {
            return rest.parse().ok();
        }
    }
    // Alias spelling: `<base><N>Size`.
    if let Some(digits) = name.strip_prefix(base).and_then(|r| r.strip_suffix("Size")) {
        if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
            return digits.parse().ok();
        }
    }
    None
}

/// Parse a per-pass output-size semantic name into its pass index, accepting
/// every spelling RetroArch uses/accepts (§7/§11): the canonical `PassOutputSizeK`
/// it emits, plus the `PassOutputKSize` and `PassKSize` alias forms. Returns
/// `None` for any other name (including a `PassFeedback…` name, which is a
/// *different* semantic handled by [`parse_pass_feedback_size_index`]).
fn parse_pass_size_index(name: &str) -> Option<usize> {
    // Reject the feedback family explicitly so `PassFeedbackSize0` is never read as
    // a pass-output size (it shares the `Pass` prefix).
    if name.starts_with("PassFeedback") {
        return None;
    }
    parse_indexed_size(name, "PassOutput").or_else(|| parse_indexed_size(name, "Pass"))
}

/// Parse a feedback-size semantic name into its pass index `K` (#24, §4),
/// accepting the canonical `PassFeedbackSizeK` RetroArch emits and the
/// `PassFeedbackKSize` alias form. Returns `None` for any other name. Distinct
/// from [`parse_pass_size_index`], which rejects the `Feedback` family.
fn parse_pass_feedback_size_index(name: &str) -> Option<usize> {
    parse_indexed_size(name, "PassFeedback")
}

/// Parse an original-history-size semantic name into its depth `K` (#25, §5),
/// accepting the canonical `OriginalHistorySizeK` RetroArch emits and the
/// `OriginalHistoryKSize` alias form, with `K = 0` (which the caller maps to
/// `OriginalSize`). Returns `None` for any other name.
fn parse_history_size_index(name: &str) -> Option<usize> {
    parse_indexed_size(name, "OriginalHistory")
}

/// Pack the builtin semantics into the byte image of one reflected uniform block
/// (#28): for every member of `block`, if its name matches a semantic in
/// `values`, write that value's bytes at the member's reflected offset. Members
/// matching no semantic (a `#pragma parameter` in a shared block — #29 — or a
/// not-yet-wired resource size) are left at their zero initialization.
///
/// The returned buffer is exactly `block.size` bytes (the std140 block size, a
/// 16-byte multiple), zero-filled where no semantic wrote — ready to upload to
/// the block's UBO/push binding. This is layout-driven: the member *order* in
/// the shader is irrelevant; only the reflected offsets matter.
pub fn pack_builtins(block: &UniformBlock, values: &BuiltinValues) -> Vec<u8> {
    let mut bytes = vec![0u8; block.size as usize];
    for m in &block.members {
        let Some(src) = values.member_bytes(&m.name) else {
            continue;
        };
        let start = m.offset as usize;
        // Never write past the member's reflected size or the block end (guards
        // against a type/semantic mismatch — e.g. a vec4 semantic into a vec2).
        let max = (m.size as usize).min(src.len());
        let end = (start + max).min(bytes.len());
        if start < bytes.len() {
            bytes[start..end].copy_from_slice(&src[..end - start]);
        }
    }
    bytes
}

/// Choose the pass's **builtin** uniform block from a reflection: the block that
/// declares a recognizable builtin semantic member (`MVP`, an `*Size`,
/// `FrameCount`, …). Returns `None` if the shader declares no builtin block
/// (e.g. a constant-color pass). The parameter block (only `#pragma parameter`
/// members) is intentionally not matched here — that is #29's concern.
pub fn builtin_block(reflection: &SpirvReflection) -> Option<&UniformBlock> {
    reflection
        .blocks
        .iter()
        .find(|b| b.members.iter().any(|m| is_builtin_semantic(&m.name)))
}

/// Whether a member name is a builtin semantic this engine recognizes (a scalar
/// semantic, an `*Size`, or a per-pass `PassKSize`/`PassOutputKSize`). Used to
/// distinguish the builtin block from a pure parameter block.
fn is_builtin_semantic(name: &str) -> bool {
    matches!(
        name,
        "MVP"
            | "SourceSize"
            | "OriginalSize"
            | "OutputSize"
            | "FinalViewportSize"
            | "FrameCount"
            | "FrameDirection"
            | "Rotation"
    ) || (name.ends_with("Size")
        && (parse_pass_size_index(name).is_some()
            || parse_pass_feedback_size_index(name).is_some()
            || parse_history_size_index(name).is_some()))
}

/// Apply a pass's `frame_count_mod` to the raw frame counter, per §6:
/// `mod > 0 ? frame_count % mod : frame_count`.
pub fn apply_frame_count_mod(frame_count: u32, frame_count_mod: u32) -> u32 {
    if frame_count_mod > 0 {
        frame_count % frame_count_mod
    } else {
        frame_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::offset_of;

    fn param(name: &str, default: f32) -> Parameter {
        Parameter {
            name: name.to_string(),
            label: name.to_string(),
            default,
            min: 0.0,
            max: 1.0,
            step: 0.0,
        }
    }

    #[test]
    fn size_vec_is_w_h_and_reciprocals() {
        assert_eq!(size_vec(4, 2), [4.0, 2.0, 0.25, 0.5]);
        // Zero dimensions clamp to 1 so the reciprocal stays finite.
        assert_eq!(size_vec(0, 0), [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn ubo_field_offsets_match_std140() {
        // The byte image must match the shader's declared block exactly.
        assert_eq!(offset_of!(BuiltinUniforms, mvp), 0);
        assert_eq!(offset_of!(BuiltinUniforms, source_size), 64);
        assert_eq!(offset_of!(BuiltinUniforms, original_size), 80);
        assert_eq!(offset_of!(BuiltinUniforms, output_size), 96);
        assert_eq!(offset_of!(BuiltinUniforms, frame_count), 112);
        // std140 rounds the block up to a multiple of 16.
        assert_eq!(std::mem::size_of::<BuiltinUniforms>(), 128);
    }

    #[test]
    fn builtins_compute_sizes_and_carry_frame_count() {
        let u = BuiltinUniforms::new((320, 240), (640, 480), 7);
        assert_eq!(u.source_size, [320.0, 240.0, 1.0 / 320.0, 1.0 / 240.0]);
        assert_eq!(u.original_size, u.source_size);
        assert_eq!(u.output_size, [640.0, 480.0, 1.0 / 640.0, 1.0 / 480.0]);
        assert_eq!(u.frame_count, 7);
        assert_eq!(u.pad, [0; 3]);
    }

    #[test]
    fn mvp_maps_unit_quad_to_clip_space() {
        let m = ortho_mvp();
        // Column-major mat4 * vec4(x, y, 0, 1).
        let apply = |x: f32, y: f32| {
            let p = [x, y, 0.0, 1.0];
            let mut out = [0.0f32; 4];
            for (row, slot) in out.iter_mut().enumerate() {
                *slot = (0..4).map(|col| m[col * 4 + row] * p[col]).sum();
            }
            (out[0], out[1])
        };
        // The unit square's corners map onto the clip-space corners.
        assert_eq!(apply(0.0, 0.0), (-1.0, -1.0));
        assert_eq!(apply(1.0, 1.0), (1.0, 1.0));
        assert_eq!(apply(0.5, 0.5), (0.0, 0.0));
    }

    #[test]
    fn parameters_pack_consecutively_and_pad_to_16() {
        let bytes = pack_parameters(&[param("A", 0.5), param("B", 0.25)]);
        assert_eq!(bytes.len(), 16); // 2 floats -> padded up to 16
        assert_eq!(&bytes[0..4], &0.5f32.to_le_bytes());
        assert_eq!(&bytes[4..8], &0.25f32.to_le_bytes());
        assert_eq!(&bytes[8..16], &[0u8; 8]); // padding is zero

        // Five floats spill into a second 16-byte stride.
        let many: Vec<_> = (0..5).map(|i| param("P", i as f32)).collect();
        assert_eq!(pack_parameters(&many).len(), 32);
    }

    #[test]
    fn no_parameters_still_yields_one_vec4() {
        // A bound UBO needs storage even when the shader declares no parameters.
        assert_eq!(pack_parameters(&[]), vec![0u8; 16]);
    }

    // ---- Reflection-driven builtin packing (#28; no GPU). ----

    use slang_compile::{BlockBinding, MemberKind, ScalarType, UniformBlock, UniformMember};

    fn member(name: &str, offset: u32, size: u32, kind: MemberKind) -> UniformMember {
        UniformMember {
            name: name.to_string(),
            offset,
            size,
            kind,
        }
    }

    fn vec4_kind() -> MemberKind {
        MemberKind::Vector {
            scalar: ScalarType::Float,
            len: 4,
        }
    }

    fn block(size: u32, members: Vec<UniformMember>) -> UniformBlock {
        UniformBlock {
            name: Some("UBO".into()),
            binding: BlockBinding::Uniform { set: 0, binding: 0 },
            size,
            members,
        }
    }

    fn f32x4(b: &[u8], off: usize) -> [f32; 4] {
        let mut out = [0.0f32; 4];
        for (i, slot) in out.iter_mut().enumerate() {
            let s = off + i * 4;
            *slot = f32::from_le_bytes([b[s], b[s + 1], b[s + 2], b[s + 3]]);
        }
        out
    }

    #[test]
    fn apply_frame_count_mod_wraps_only_when_positive() {
        assert_eq!(apply_frame_count_mod(7, 0), 7); // mod 0 -> unmodified
        assert_eq!(apply_frame_count_mod(7, 4), 3); // 7 % 4
        assert_eq!(apply_frame_count_mod(8, 4), 0); // wraps to 0
    }

    #[test]
    fn parse_pass_size_index_accepts_both_spellings() {
        // Canonical RetroArch uniform spelling (`<base>Size<N>`, Size BEFORE the
        // number) — what `slang_process.cpp` emits and the corpus actually uses.
        assert_eq!(parse_pass_size_index("PassOutputSize0"), Some(0));
        assert_eq!(parse_pass_size_index("PassOutputSize1"), Some(1));
        // Alias spellings (Size AFTER the number) RetroArch also accepts.
        assert_eq!(parse_pass_size_index("Pass0Size"), Some(0));
        assert_eq!(parse_pass_size_index("PassOutput0Size"), Some(0));
        assert_eq!(parse_pass_size_index("Pass12Size"), Some(12));
        assert_eq!(parse_pass_size_index("PassOutput3Size"), Some(3));
        // Not a pass-output size: feedback is a distinct semantic (its own parser),
        // and must NOT be read here in either spelling.
        assert_eq!(parse_pass_size_index("PassFeedbackSize0"), None);
        assert_eq!(parse_pass_size_index("PassFeedback0Size"), None);
        // No index / no Size suffix.
        assert_eq!(parse_pass_size_index("PassOutputSize"), None);
        assert_eq!(parse_pass_size_index("Pass0"), None);
        assert_eq!(parse_pass_size_index("SourceSize"), None);
    }

    #[test]
    fn parse_feedback_and_history_size_accept_retroarch_spelling() {
        // The bug #32 closed: RetroArch emits `PassFeedbackSize<N>` /
        // `OriginalHistorySize<N>` (Size BEFORE the number); the old parsers only
        // accepted the `<base><N>Size` alias, so `feedback.slang`'s
        // `PassFeedbackSize0` member was left zero — making the shader sample texel
        // (0,0) everywhere and "accumulate to white".
        assert_eq!(parse_pass_feedback_size_index("PassFeedbackSize0"), Some(0));
        assert_eq!(parse_pass_feedback_size_index("PassFeedbackSize2"), Some(2));
        assert_eq!(parse_pass_feedback_size_index("PassFeedback0Size"), Some(0)); // alias
        assert_eq!(parse_pass_feedback_size_index("PassOutputSize0"), None);

        assert_eq!(parse_history_size_index("OriginalHistorySize1"), Some(1));
        assert_eq!(parse_history_size_index("OriginalHistorySize5"), Some(5));
        assert_eq!(parse_history_size_index("OriginalHistory1Size"), Some(1)); // alias
        assert_eq!(parse_history_size_index("OriginalHistorySize0"), Some(0));
    }

    #[test]
    fn pack_builtins_writes_each_semantic_at_its_reflected_offset() {
        // A NON-canonical order + subset: FrameCount before the sizes, no MVP.
        let blk = block(
            48,
            vec![
                member("FrameCount", 0, 4, MemberKind::Scalar(ScalarType::Uint)),
                member("OutputSize", 16, 16, vec4_kind()),
                member("SourceSize", 32, 16, vec4_kind()),
            ],
        );
        let values = BuiltinValues {
            source_size: size_vec(320, 240),
            output_size: size_vec(640, 480),
            frame_count: 9,
            ..Default::default()
        };
        let bytes = pack_builtins(&blk, &values);
        assert_eq!(bytes.len(), 48);
        // FrameCount at offset 0.
        assert_eq!(
            u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            9
        );
        // OutputSize at 16, SourceSize at 32 — proves offset-by-name, not order.
        assert_eq!(f32x4(&bytes, 16), size_vec(640, 480));
        assert_eq!(f32x4(&bytes, 32), size_vec(320, 240));
    }

    #[test]
    fn pack_builtins_leaves_unknown_members_zero() {
        // A member matching no semantic (a #pragma parameter living in a shared
        // block — #29's concern) must be left untouched.
        let blk = block(
            32,
            vec![
                member("OutputSize", 0, 16, vec4_kind()),
                member("USER_PARAM", 16, 4, MemberKind::Scalar(ScalarType::Float)),
            ],
        );
        let values = BuiltinValues {
            output_size: size_vec(100, 50),
            ..Default::default()
        };
        let bytes = pack_builtins(&blk, &values);
        assert_eq!(f32x4(&bytes, 0), size_vec(100, 50));
        // The parameter slot stays zero (untouched by builtin packing).
        assert_eq!(&bytes[16..20], &[0u8; 4]);
    }

    #[test]
    fn pack_builtins_handles_pass_output_size_both_spellings() {
        let values = BuiltinValues {
            pass_output_sizes: vec![size_vec(64, 64), size_vec(128, 96)],
            ..Default::default()
        };
        // `Pass0Size` and `PassOutput1Size` resolve to passes 0 and 1.
        let blk = block(
            32,
            vec![
                member("Pass0Size", 0, 16, vec4_kind()),
                member("PassOutput1Size", 16, 16, vec4_kind()),
            ],
        );
        let bytes = pack_builtins(&blk, &values);
        assert_eq!(f32x4(&bytes, 0), size_vec(64, 64));
        assert_eq!(f32x4(&bytes, 16), size_vec(128, 96));

        // A PassNSize past the known passes stays zero (causal / not-yet-run).
        let blk2 = block(16, vec![member("Pass5Size", 0, 16, vec4_kind())]);
        assert_eq!(pack_builtins(&blk2, &values), vec![0u8; 16]);
    }

    #[test]
    fn member_bytes_resolves_alias_size() {
        // `<alias>Size` resolves to the aliased pass's output size (#26); an
        // un-recorded alias leaves the member zero (returns None).
        let mut alias_sizes = std::collections::HashMap::new();
        alias_sizes.insert("FOO".to_string(), size_vec(100, 60));
        let values = BuiltinValues {
            alias_sizes,
            ..Default::default()
        };
        let bytes = values.member_bytes("FOOSize").expect("FOOSize resolves");
        let got = f32x4(&bytes, 0);
        assert_eq!(got, size_vec(100, 60));
        // An alias the chain didn't record stays unknown.
        assert!(values.member_bytes("BARSize").is_none());
    }

    #[test]
    fn member_bytes_resolves_known_semantics_and_rejects_unknown() {
        let values = BuiltinValues {
            frame_direction: -1,
            rotation: 2,
            ..Default::default()
        };
        assert_eq!(
            values.member_bytes("FrameDirection"),
            Some((-1i32).to_le_bytes().to_vec())
        );
        assert_eq!(
            values.member_bytes("Rotation"),
            Some(2u32.to_le_bytes().to_vec())
        );
        assert!(values.member_bytes("MVP").is_some());
        // A recognized history-size semantic whose ring slot is empty returns None
        // (member left zero) — in BOTH the RetroArch and alias spellings.
        assert!(values.member_bytes("OriginalHistorySize1").is_none());
        assert!(values.member_bytes("OriginalHistory1Size").is_none());
        // A truly unknown member name also returns None.
        assert!(values.member_bytes("SomeUserParam").is_none());
    }

    // ---- Reflection-driven parameter packing + state (#29; no GPU). ----

    fn full_param(name: &str, default: f32, min: f32, max: f32) -> Parameter {
        Parameter {
            name: name.to_string(),
            label: format!("{name} label"),
            default,
            min,
            max,
            step: 0.1,
        }
    }

    #[test]
    fn param_store_collects_globally_by_name_and_seeds_defaults() {
        // Two passes; param X declared in both (same value), Y only in pass 1.
        // Declaration order is preserved; X is collected once (global by name).
        let pass0 = vec![full_param("X", 0.5, 0.0, 1.0)];
        let pass1 = vec![
            full_param("X", 0.5, 0.0, 1.0),
            full_param("Y", 2.0, 0.0, 4.0),
        ];
        let aliases = std::collections::HashMap::new();
        let store = ParamStore::collect([pass0.as_slice(), pass1.as_slice()], &aliases);

        let views = store.views();
        assert_eq!(views.len(), 2, "X deduped, Y added");
        assert_eq!(views[0].name, "X");
        assert_eq!(views[0].value, 0.5);
        assert_eq!(views[1].name, "Y");
        assert_eq!(views[1].value, 2.0);
        // Lookups by name return the current (default) values.
        assert_eq!(store.value("X"), Some(0.5));
        assert_eq!(store.value("Y"), Some(2.0));
        assert_eq!(store.value("Z"), None);
    }

    #[test]
    fn param_store_stores_raw_and_clamps_at_use() {
        // §11 item 7: `set` stores the RAW value in `current`; the clamp to
        // `[min, max]` happens only at use (in `clamped_value`/`pack_params`).
        let p = vec![full_param("LEVEL", 0.5, 0.0, 1.0)];
        let aliases = std::collections::HashMap::new();
        let mut store = ParamStore::collect([p.as_slice()], &aliases);

        assert!(store.set("LEVEL", 0.75));
        assert_eq!(store.value("LEVEL"), Some(0.75));
        assert_eq!(store.clamped_value("LEVEL"), Some(0.75));
        // Above max: raw stays 5.0; the clamped (use-time) value is the max.
        assert!(store.set("LEVEL", 5.0));
        assert_eq!(store.value("LEVEL"), Some(5.0), "raw value is unclamped");
        assert_eq!(store.clamped_value("LEVEL"), Some(1.0), "clamped at use");
        // Below min: raw stays -3.0; clamped to the min.
        assert!(store.set("LEVEL", -3.0));
        assert_eq!(store.value("LEVEL"), Some(-3.0), "raw value is unclamped");
        assert_eq!(store.clamped_value("LEVEL"), Some(0.0), "clamped at use");
        // An unknown name is a no-op.
        assert!(!store.set("NOPE", 0.5));
    }

    #[test]
    fn pack_params_clamps_the_packed_value() {
        // The raw stored value may be out of range, but the PACKED bytes must be
        // clamped to `[min, max]` (§11 item 7) so the rendered pixel stays in range.
        let blk = block(
            16,
            vec![member("LEVEL", 0, 4, MemberKind::Scalar(ScalarType::Float))],
        );
        let p = vec![full_param("LEVEL", 0.5, 0.0, 1.0)];
        let aliases = std::collections::HashMap::new();
        let mut store = ParamStore::collect([p.as_slice()], &aliases);
        assert!(store.set("LEVEL", 5.0)); // raw 5.0, out of range
        assert_eq!(store.value("LEVEL"), Some(5.0), "raw unclamped");

        let mut bytes = vec![0u8; 16];
        pack_params(&mut bytes, &blk, &store);
        // Packed value is clamped to the max (1.0), NOT the raw 5.0.
        assert_eq!(&bytes[0..4], &1.0f32.to_le_bytes(), "packed value clamped");
    }

    #[test]
    fn param_store_alias_drives_the_same_value() {
        let p = vec![full_param("BRIGHT", 1.0, 0.0, 2.0)];
        let mut aliases = std::collections::HashMap::new();
        aliases.insert("BRIGHT".to_string(), "brightness".to_string());
        let mut store = ParamStore::collect([p.as_slice()], &aliases);

        // Setting via the alias updates the canonical value (and vice versa).
        assert!(store.set("brightness", 1.5));
        assert_eq!(store.value("BRIGHT"), Some(1.5));
        assert_eq!(store.value("brightness"), Some(1.5));
        assert!(store.set("BRIGHT", 0.25));
        assert_eq!(store.value("brightness"), Some(0.25));
    }

    #[test]
    fn param_store_overrides_set_current_not_default() {
        let p = vec![full_param("CONTRAST", 0.5, 0.0, 1.0)];
        let aliases = std::collections::HashMap::new();
        let mut store = ParamStore::collect([p.as_slice()], &aliases);

        let mut overrides = std::collections::BTreeMap::new();
        overrides.insert("CONTRAST".to_string(), 0.9);
        // An out-of-range override is also clamped.
        overrides.insert("UNKNOWN".to_string(), 1.0);
        store.apply_overrides(&overrides);

        assert_eq!(store.value("CONTRAST"), Some(0.9));
        // The pragma default is untouched (visible if we never overrode it).
        assert_eq!(store.views()[0].value, 0.9);
        assert_eq!(store.value("UNKNOWN"), None, "unknown override ignored");
    }

    #[test]
    fn pack_params_writes_current_value_at_member_offset() {
        // A param-only block with members in NON-canonical order: B at 0, A at 4.
        let blk = block(
            16,
            vec![
                member("B", 0, 4, MemberKind::Scalar(ScalarType::Float)),
                member("A", 4, 4, MemberKind::Scalar(ScalarType::Float)),
            ],
        );
        let params = vec![
            full_param("A", 0.25, 0.0, 1.0),
            full_param("B", 0.75, 0.0, 1.0),
        ];
        let aliases = std::collections::HashMap::new();
        let store = ParamStore::collect([params.as_slice()], &aliases);

        let mut bytes = vec![0u8; 16];
        pack_params(&mut bytes, &blk, &store);
        // B at offset 0, A at offset 4 — proves offset-by-name, not declaration
        // order in the #pragma list.
        assert_eq!(&bytes[0..4], &0.75f32.to_le_bytes());
        assert_eq!(&bytes[4..8], &0.25f32.to_le_bytes());
    }

    #[test]
    fn pack_params_overlays_builtins_in_a_mixed_block() {
        // A block mixing a builtin (OutputSize) and a param (LEVEL): pack_builtins
        // then pack_params must each land at their own offset without clobbering.
        let blk = block(
            32,
            vec![
                member("OutputSize", 0, 16, vec4_kind()),
                member("LEVEL", 16, 4, MemberKind::Scalar(ScalarType::Float)),
            ],
        );
        let values = BuiltinValues {
            output_size: size_vec(100, 50),
            ..Default::default()
        };
        let params = vec![full_param("LEVEL", 0.5, 0.0, 1.0)];
        let aliases = std::collections::HashMap::new();
        let store = ParamStore::collect([params.as_slice()], &aliases);

        let mut bytes = pack_builtins(&blk, &values);
        pack_params(&mut bytes, &blk, &store);
        assert_eq!(f32x4(&bytes, 0), size_vec(100, 50), "builtin survives");
        assert_eq!(&bytes[16..20], &0.5f32.to_le_bytes(), "param packed");
    }

    #[test]
    fn pack_params_skips_a_param_colliding_with_a_builtin_name() {
        // A `#pragma parameter` named like a builtin (`OutputSize`) must NOT
        // overwrite the builtin the renderer already packed: the builtin wins
        // (#28/#29). pack_params skips members whose name is a builtin semantic.
        let blk = block(16, vec![member("OutputSize", 0, 16, vec4_kind())]);
        let mut bytes = pack_builtins(
            &blk,
            &BuiltinValues {
                output_size: size_vec(640, 480),
                ..Default::default()
            },
        );
        // A param colliding with the builtin name; its value would clobber if not
        // skipped (the param is a scalar but would still write its 4 bytes).
        let params = vec![full_param("OutputSize", 0.0, 0.0, 1.0)];
        let aliases = std::collections::HashMap::new();
        let store = ParamStore::collect([params.as_slice()], &aliases);
        pack_params(&mut bytes, &blk, &store);
        // The builtin OutputSize survives untouched (param did NOT overwrite it).
        assert_eq!(
            f32x4(&bytes, 0),
            size_vec(640, 480),
            "builtin wins over param"
        );
    }

    #[test]
    fn pack_params_is_noop_for_empty_store() {
        let blk = block(
            16,
            vec![member("X", 0, 4, MemberKind::Scalar(ScalarType::Float))],
        );
        let mut bytes = vec![7u8; 16];
        pack_params(&mut bytes, &blk, &ParamStore::default());
        assert_eq!(bytes, vec![7u8; 16], "empty store touches nothing");
    }

    #[test]
    fn builtin_block_picks_the_block_with_a_builtin_member() {
        use slang_compile::SpirvReflection;
        let builtin = block(16, vec![member("OutputSize", 0, 16, vec4_kind())]);
        let params = UniformBlock {
            name: Some("Params".into()),
            binding: BlockBinding::Uniform { set: 0, binding: 3 },
            size: 16,
            members: vec![member("LEVEL", 0, 4, MemberKind::Scalar(ScalarType::Float))],
        };
        let reflection = SpirvReflection {
            blocks: vec![params.clone(), builtin.clone()],
            ..Default::default()
        };
        // The builtin block is found; the pure-parameter block is not mistaken
        // for it.
        let found = builtin_block(&reflection).expect("builtin block");
        assert!(found.member("OutputSize").is_some());
        assert!(found.member("LEVEL").is_none());
    }
}
