//! Reflection-driven per-pass texture bind table (#26): turning the RetroArch
//! texture semantics a pass *declares* into the live `wgpu::TextureView` +
//! `wgpu::Sampler` each binding needs, and a resolver hook for the resources
//! that don't exist yet (`docs/retroarch-slang-runtime.md` §7).
//!
//! ## Why this exists
//!
//! Before #26 the renderer used a single FIXED bind-group layout (b0 builtin UBO,
//! b1 `Source` texture, b2 sampler, b3 param UBO) and chained pass `i`'s input
//! from pass `i-1`'s FBO. That works only for the one-texture `Source` case. Real
//! slang passes sample `Original`, `PassOutputN`/`PassN`, an `<alias>`, plus
//! deferred resources (`PassFeedbackN`, `OriginalHistoryN`, LUTs). This module
//! makes the bind layout and the resolved textures **come from the pass's
//! reflection** instead of a hard-coded set.
//!
//! ## The two halves
//!
//! 1. [`pass_layout`] builds a pass's [`wgpu::BindGroupLayout`] from its
//!    [`SpirvReflection`]: every reflected uniform block (UBO or push) at its
//!    reflected binding, plus every reflected texture and sampler at its reflected
//!    binding. The existing separate-sampler fixtures (UBO@b0, Source@b1, Smp@b2,
//!    Params@b3) reflect to exactly the legacy layout, so they keep working.
//!
//! 2. [`TextureResolver`] is the **extension hook** #24 (feedback), #25 (history),
//!    and #27 (LUTs) plug into. For each reflected texture *name*, the renderer
//!    asks the resolver for the live resource. The names the engine can satisfy
//!    *today* (`Source`/`Original`/`PassOutputN`/`PassN`/`<alias>`) are resolved
//!    by the renderer itself (see `renderer::rebuild_chain`); everything else is
//!    handed to the resolver, which currently returns a placeholder so a binding
//!    never fails. #24/#25/#27 will replace [`PlaceholderResolver`] with real
//!    feedback/history/LUT lookups **without touching the layout or bind-group
//!    plumbing** — they only fill in [`TextureResolver::resolve`].
//!
//! ## Texture name → resource mapping (§7), as implemented in #26
//!
//! | Name                | Resolved by | To |
//! |---------------------|-------------|----|
//! | `Source`            | renderer    | pass `i-1`'s output FBO (pass 0: the source image) |
//! | `Original`          | renderer    | the source image (any pass) |
//! | `PassOutputN`/`PassN` | renderer  | pass `N`'s output FBO (causal: `N < i`) |
//! | `<alias>`           | renderer    | the output of the pass whose preset `aliasN == <alias>` |
//! | `PassFeedbackN`/`<alias>Feedback` | resolver | **placeholder** (→ #24) |
//! | `OriginalHistoryN`  | resolver    | **placeholder** (→ #25) |
//! | LUT `<NAME>`/`UserN`| resolver    | **placeholder** (→ #27) |
//!
//! Sampler attribution (§3/§7, libretro/RetroArch#14437): a texture produced by
//! pass `K` is sampled with the `filter_linear`/`wrap_mode`/`mipmap_input` of pass
//! `K+1` (the pass *immediately after* its producer) — **not** the producer and
//! **not** the (later) consumer. For a pass's direct `Source` (= pass `i-1`'s
//! output, `K = i-1`) that resolves to pass `i`'s own config (so #23's per-pass
//! sampler is preserved); `Original`/pass-0 `Source` ("produced by pass -1") uses
//! pass 0's config. This bookkeeping lives in `renderer::resolve_texture`; this
//! module only classifies names and builds the layout.

use slang_compile::{BlockBinding, SpirvReflection};

/// What a reflected texture name refers to, after stripping any trailing index.
/// The renderer satisfies the [`TextureClass::is_resolved_by_renderer`] cases
/// from resources it already owns; the rest are handed to a [`TextureResolver`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextureClass {
    /// `Source` — this pass's input (pass 0 = the source image; pass `i` = pass
    /// `i-1`'s output).
    Source,
    /// `Original` — the whole-chain input image, bindable by **any** pass.
    Original,
    /// `PassOutputN` / the `PassN` alias — pass `N`'s output **this frame**.
    /// Causal: `N` must be `< i` or the bind is unsatisfiable.
    PassOutput(usize),
    /// `<alias>` — the output of the pass whose preset `aliasN` equals this name.
    /// Resolved against the chain's alias table (causal, like `PassOutput`).
    Alias(String),
    /// `PassFeedbackN` — pass `N`'s output from the **previous** frame (§4). Not
    /// implemented yet; routed to the resolver placeholder (→ #24).
    PassFeedback(usize),
    /// `<alias>Feedback` — the aliased pass's previous-frame output (§4). Deferred
    /// to the resolver placeholder (→ #24).
    AliasFeedback(String),
    /// `OriginalHistoryN` — the source frame `N` frames ago (§5). Deferred to the
    /// resolver placeholder (→ #25). `OriginalHistory0 ≡ Original`.
    OriginalHistory(usize),
    /// A LUT (`textures=` entry) or the un-aliased `UserN` fallback (§7). Deferred
    /// to the resolver placeholder (→ #27).
    Lut(String),
}

impl TextureClass {
    /// Classify a reflected texture's GLSL name into a [`TextureClass`] (§7
    /// mapping rules). `aliases` is the chain's set of pass `#pragma name` /
    /// preset `aliasN` identifiers; a name in it (or its `…Feedback` form) takes
    /// precedence over the generic `UserN`/LUT fallback (§7 rule 3).
    ///
    /// The classification is deliberately total: an unrecognized name falls
    /// through to [`TextureClass::Lut`] so it routes to the resolver placeholder
    /// rather than failing the bind — a LUT/user texture the renderer doesn't
    /// model yet (#27).
    pub fn classify(name: &str, aliases: &[String]) -> TextureClass {
        // Exact, un-indexed semantics first.
        match name {
            "Source" => return TextureClass::Source,
            "Original" => return TextureClass::Original,
            _ => {}
        }
        // Alias precedence (§7 rule 3): `<alias>` and `<alias>Feedback`.
        for a in aliases {
            if name == a {
                return TextureClass::Alias(a.clone());
            }
            if let Some(base) = name.strip_suffix("Feedback") {
                if base == a {
                    return TextureClass::AliasFeedback(a.clone());
                }
            }
        }
        // Indexed semantics: strip the trailing digits → base + index.
        if let Some((base, idx)) = split_indexed(name) {
            match base {
                "PassOutput" | "Pass" => return TextureClass::PassOutput(idx),
                "PassFeedback" => return TextureClass::PassFeedback(idx),
                "OriginalHistory" => {
                    // `OriginalHistory0 ≡ Original` (§5) — bind the live source.
                    return if idx == 0 {
                        TextureClass::Original
                    } else {
                        TextureClass::OriginalHistory(idx)
                    };
                }
                "User" => return TextureClass::Lut(name.to_string()),
                _ => {}
            }
        }
        // Anything else: a LUT name or unmodeled user texture (→ #27 placeholder).
        TextureClass::Lut(name.to_string())
    }

    /// Whether the renderer resolves this class itself from resources that exist
    /// today (`Source`/`Original`/`PassOutputN`/`<alias>`). The complementary
    /// classes (`false`) route to the [`TextureResolver`] hook.
    pub fn is_resolved_by_renderer(&self) -> bool {
        matches!(
            self,
            TextureClass::Source
                | TextureClass::Original
                | TextureClass::PassOutput(_)
                | TextureClass::Alias(_)
        )
    }
}

/// Split a texture name into its base semantic and trailing decimal index, e.g.
/// `PassOutput3` → `("PassOutput", 3)`, `OriginalHistory1` → `("OriginalHistory",
/// 1)`. Returns `None` when there is no trailing digit run.
fn split_indexed(name: &str) -> Option<(&str, usize)> {
    let split = name.bytes().rposition(|b| !b.is_ascii_digit())? + 1;
    if split >= name.len() {
        return None; // no trailing digits
    }
    let (base, digits) = name.split_at(split);
    digits.parse().ok().map(|idx| (base, idx))
}

/// The extension hook #24 (feedback) / #25 (history) / #27 (LUTs) implement.
///
/// For every reflected texture a pass declares that the renderer does **not**
/// resolve itself (see [`TextureClass::is_resolved_by_renderer`]), the renderer
/// calls [`TextureResolver::resolve`] with the classified semantic. The resolver
/// returns the live [`wgpu::TextureView`] for that resource (and, implicitly, the
/// renderer pairs it with a default sampler — feedback/history follow the source
/// sampler defaults; LUTs will carry their own in #27).
///
/// The default [`PlaceholderResolver`] returns a shared 1×1 black texture for
/// every deferred class so a bind group is always complete and validation never
/// fails on a cold/unimplemented resource — matching the spec's "cold frames read
/// transparent black" (§4/§5). #24/#25/#27 swap in a resolver that returns the
/// real feedback/history/LUT views; **no other code changes** because the layout
/// and bind-group assembly already route every deferred name here.
pub trait TextureResolver {
    /// Resolve a deferred texture class to a live view. `_pass_index` is the
    /// consuming pass (so a future resolver can honor causality / per-pass
    /// history depth). Returning `None` means "no resource" — the renderer then
    /// falls back to the placeholder so the bind still succeeds.
    fn resolve(&self, class: &TextureClass, pass_index: usize) -> Option<&wgpu::TextureView>;
}

/// The default resolver (#26): every deferred class (`PassFeedbackN`,
/// `OriginalHistoryN`, LUTs) resolves to a shared 1×1 black texture so binding
/// never fails before #24/#25/#27 land. Holds the placeholder view.
pub struct PlaceholderResolver {
    black: wgpu::TextureView,
}

impl PlaceholderResolver {
    /// Build the resolver with a freshly-created 1×1 opaque-black placeholder
    /// texture (matching the §4/§5 cold-start "transparent/opaque black" read).
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bindtable placeholder (1x1 black)"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[0, 0, 0, 0],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let black = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self { black }
    }

    /// The shared 1×1 black view, used as the fallback whenever a class can't be
    /// resolved (deferred resource, or a future resolver returning `None`).
    pub fn black(&self) -> &wgpu::TextureView {
        &self.black
    }
}

impl TextureResolver for PlaceholderResolver {
    fn resolve(&self, _class: &TextureClass, _pass_index: usize) -> Option<&wgpu::TextureView> {
        // Every deferred class maps to the shared black placeholder until
        // #24/#25/#27 provide the real resource.
        Some(&self.black)
    }
}

/// Build a pass's bind-group layout from its SPIR-V reflection (#26): one entry
/// per reflected uniform block at its reflected binding (a filtering UBO), plus
/// one entry per reflected texture (a filterable 2D float texture) and one per
/// reflected sampler (a filtering sampler), each at its own reflected binding.
///
/// All current bindings live in descriptor **set 0** (the only set the slang
/// toolchain emits for these shaders); a binding in another set is skipped with a
/// warning rather than silently mis-bound. The pipeline layout is then built from
/// this single set-0 layout.
///
/// The legacy fixtures (UBO@b0, Source@b1 texture, Smp@b2 sampler, Params@b3 UBO)
/// reflect to exactly the four entries the old fixed layout had, so they bind and
/// validate identically.
pub fn pass_layout(device: &wgpu::Device, reflection: &SpirvReflection) -> wgpu::BindGroupLayout {
    let mut entries: Vec<wgpu::BindGroupLayoutEntry> = Vec::new();

    for block in &reflection.blocks {
        // Only set-0 UBOs get a descriptor entry. Push-constant blocks bind
        // through the pipeline's immediate range, not a bind group, so they have
        // no layout entry here. (The current toolchain emits the builtin/param
        // blocks as set-0 UBOs.)
        if let BlockBinding::Uniform { set, binding } = block.binding {
            if set != 0 {
                eprintln!(
                    "preview-engine: uniform block {:?} is in set {set} (binding {binding}); \
                     only set 0 is supported — skipping its layout entry",
                    block.name
                );
                continue;
            }
            entries.push(wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            });
        }
    }

    for tex in &reflection.textures {
        if tex.set != 0 {
            eprintln!(
                "preview-engine: texture {:?} is in set {} (binding {}); only set 0 is \
                 supported — skipping",
                tex.name, tex.set, tex.binding
            );
            continue;
        }
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: tex.binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        });
    }

    for smp in &reflection.samplers {
        if smp.set != 0 {
            eprintln!(
                "preview-engine: sampler {:?} is in set {} (binding {}); only set 0 is \
                 supported — skipping",
                smp.name, smp.set, smp.binding
            );
            continue;
        }
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: smp.binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        });
    }

    // wgpu requires layout entries sorted by binding for some validators; sort to
    // be safe (the bind group entries match by binding number regardless).
    entries.sort_by_key(|e| e.binding);

    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("pass bind group layout (reflection-driven)"),
        entries: &entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_direct_semantics() {
        let none: &[String] = &[];
        assert_eq!(TextureClass::classify("Source", none), TextureClass::Source);
        assert_eq!(
            TextureClass::classify("Original", none),
            TextureClass::Original
        );
    }

    #[test]
    fn classify_pass_output_both_spellings() {
        let none: &[String] = &[];
        assert_eq!(
            TextureClass::classify("PassOutput0", none),
            TextureClass::PassOutput(0)
        );
        assert_eq!(
            TextureClass::classify("Pass2", none),
            TextureClass::PassOutput(2)
        );
        assert_eq!(
            TextureClass::classify("PassOutput12", none),
            TextureClass::PassOutput(12)
        );
    }

    #[test]
    fn classify_history_zero_is_original() {
        let none: &[String] = &[];
        // OriginalHistory0 ≡ Original (§5): renderer-resolved, not deferred.
        assert_eq!(
            TextureClass::classify("OriginalHistory0", none),
            TextureClass::Original
        );
        assert_eq!(
            TextureClass::classify("OriginalHistory1", none),
            TextureClass::OriginalHistory(1)
        );
    }

    #[test]
    fn classify_feedback_and_aliases() {
        let aliases = vec!["FOO".to_string()];
        assert_eq!(
            TextureClass::classify("FOO", &aliases),
            TextureClass::Alias("FOO".to_string())
        );
        assert_eq!(
            TextureClass::classify("FOOFeedback", &aliases),
            TextureClass::AliasFeedback("FOO".to_string())
        );
        assert_eq!(
            TextureClass::classify("PassFeedback1", &aliases),
            TextureClass::PassFeedback(1)
        );
    }

    #[test]
    fn classify_unknown_falls_through_to_lut() {
        let none: &[String] = &[];
        // A LUT/user texture the engine doesn't model yet routes to the resolver.
        assert_eq!(
            TextureClass::classify("BORDER", none),
            TextureClass::Lut("BORDER".to_string())
        );
        assert_eq!(
            TextureClass::classify("User0", none),
            TextureClass::Lut("User0".to_string())
        );
    }

    #[test]
    fn renderer_resolved_set() {
        assert!(TextureClass::Source.is_resolved_by_renderer());
        assert!(TextureClass::Original.is_resolved_by_renderer());
        assert!(TextureClass::PassOutput(0).is_resolved_by_renderer());
        assert!(TextureClass::Alias("X".into()).is_resolved_by_renderer());
        // Deferred classes route to the resolver hook.
        assert!(!TextureClass::PassFeedback(0).is_resolved_by_renderer());
        assert!(!TextureClass::OriginalHistory(1).is_resolved_by_renderer());
        assert!(!TextureClass::Lut("BORDER".into()).is_resolved_by_renderer());
    }

    #[test]
    fn split_indexed_parses_trailing_digits() {
        assert_eq!(split_indexed("PassOutput3"), Some(("PassOutput", 3)));
        assert_eq!(split_indexed("Pass0"), Some(("Pass", 0)));
        assert_eq!(
            split_indexed("OriginalHistory12"),
            Some(("OriginalHistory", 12))
        );
        assert_eq!(split_indexed("Source"), None);
        assert_eq!(split_indexed("Original"), None);
    }
}
