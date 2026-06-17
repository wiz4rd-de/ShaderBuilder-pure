//! Engine-facing per-pass description and scale-type FBO sizing
//! (`docs/retroarch-slang-runtime.md` §2). This is the *input* to the multi-pass
//! renderer: a compiled shader plus how big its render target should be. The GPU
//! resources themselves (textures, pipelines, bind groups) are built from these
//! by [`crate::renderer::Renderer`].
//!
//! Phase 2 / #22 consumes the scale config and shader. Fields for later tickets
//! (format selection #23, sampler state #23, feedback #24) are reserved here as
//! defaulted fields so the descriptor and the chain-setting API do not change
//! shape when those tickets land.

use slang_compile::CompiledShader;

/// How a pass's FBO size is derived from the available size inputs (§2).
///
/// Mirrors `preset_io::ScaleType` but lives in the engine so `preview-engine`
/// has no compile dependency on the preset parser (the app converts at the
/// boundary). The `None`/absent case is represented by [`AxisScale`] being
/// absent on the [`Pass`], not by a variant here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleType {
    /// Factor × this pass's input size (`OriginalSize` for pass 0, else the
    /// previous FBO size).
    Source,
    /// Factor × the simulated final viewport size.
    Viewport,
    /// A literal integer pixel count; the input/viewport are ignored.
    Absolute,
}

/// Sampler wrap mode for a pass's input texture (§3
/// `video_shader_wrap_str_to_mode`). Mirrors `preset_io::WrapMode` so the engine
/// has no compile dependency on the preset parser; the app converts at the
/// boundary. The §3/§11 v1 default is [`WrapMode::ClampToBorder`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrapMode {
    /// `clamp_to_border` — RetroArch `RARCH_WRAP_BORDER`, the default. Maps to
    /// `wgpu::AddressMode::ClampToBorder` **iff** the device supports
    /// `ADDRESS_MODE_CLAMP_TO_BORDER`; otherwise the renderer falls back to
    /// `ClampToEdge` (RetroArch's border is transparent-black-ish; see
    /// `renderer::address_mode`).
    ClampToBorder,
    /// `clamp_to_edge` — RetroArch `RARCH_WRAP_EDGE`.
    ClampToEdge,
    /// `repeat` — RetroArch `RARCH_WRAP_REPEAT`.
    Repeat,
    /// `mirrored_repeat` — RetroArch `RARCH_WRAP_MIRRORED_REPEAT`.
    MirroredRepeat,
}

impl Default for WrapMode {
    /// The §3/§11 v1 default: `clamp_to_border` (RetroArch fidelity).
    fn default() -> Self {
        WrapMode::ClampToBorder
    }
}

/// One axis of a scale specification: a type plus its factor. For `Absolute` the
/// factor is the literal pixel count (rounded); for `Source`/`Viewport` it
/// multiplies the relevant size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisScale {
    /// The scale type for this axis.
    pub ty: ScaleType,
    /// The scale factor (a multiplier for `Source`/`Viewport`; a literal pixel
    /// count for `Absolute`).
    pub factor: f32,
}

impl AxisScale {
    /// `source × 1.0` — the implicit default for an intermediate pass with no
    /// scale keys (§2): the FBO matches its input size.
    pub const SOURCE_1X: AxisScale = AxisScale {
        ty: ScaleType::Source,
        factor: 1.0,
    };

    /// `viewport × 1.0` — the implicit default for the last pass when it renders
    /// to its own FBO (§2): the FBO matches the viewport.
    pub const VIEWPORT_1X: AxisScale = AxisScale {
        ty: ScaleType::Viewport,
        factor: 1.0,
    };

    /// Resolve this axis to a concrete pixel size given the pass `input` size and
    /// the simulated `viewport` size along this axis (§2):
    ///
    /// ```text
    /// source   -> input    * factor
    /// viewport -> viewport * factor
    /// absolute -> factor                  (literal; input/viewport ignored)
    /// size = clamp(round(raw), 1, 16384)
    /// ```
    pub fn resolve(self, input: u32, viewport: u32) -> u32 {
        let raw = match self.ty {
            ScaleType::Source => input as f32 * self.factor,
            ScaleType::Viewport => viewport as f32 * self.factor,
            ScaleType::Absolute => self.factor,
        };
        clamp_texel(raw.round())
    }
}

/// Per-axis scale specification for a pass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScaleConfig {
    /// X-axis scale.
    pub x: AxisScale,
    /// Y-axis scale.
    pub y: AxisScale,
}

impl ScaleConfig {
    /// Resolve both axes to a concrete `(width, height)` FBO size given the pass
    /// `input` and the simulated `viewport` sizes (§2). Each axis is rounded and
    /// clamped to `[1, 16384]` independently.
    pub fn resolve(self, input: (u32, u32), viewport: (u32, u32)) -> (u32, u32) {
        (
            self.x.resolve(input.0, viewport.0),
            self.y.resolve(input.1, viewport.1),
        )
    }
}

/// `MAX_TEXEL_SIZE` — RetroArch's upper bound on an FBO dimension (§2).
pub const MAX_TEXEL_SIZE: f32 = 16384.0;

/// Clamp a rounded raw size into the valid `[1, 16384]` texel range (§2). Input
/// is already `round()`ed; this just bounds it and converts to `u32`.
fn clamp_texel(rounded: f32) -> u32 {
    rounded.clamp(1.0, MAX_TEXEL_SIZE) as u32
}

/// A pass in the engine's render chain: the compiled shader and how to size its
/// render target. `scale = None` means "no scale keys" — the renderer applies
/// the §2 position-dependent default (intermediate `source × 1.0`, last
/// `viewport`) when it builds the chain.
///
/// The reserved-for-later fields keep the descriptor (and the chain-setting API)
/// stable across Phase 2 tickets; #22 leaves them at their defaults.
#[derive(Debug, Clone)]
pub struct Pass {
    /// The compiled SPIR-V + reflection for this pass.
    pub shader: CompiledShader,
    /// Explicit scale config, or `None` to take the position default (§2).
    pub scale: Option<ScaleConfig>,
    /// `aliasN` / `#pragma name` — the pass's semantic name (#26). When set, a
    /// later pass can sample this pass's output as `<alias>` (and read its
    /// `<alias>Size`); `<alias>Feedback` lands in #24. `None` = no alias.
    pub alias: Option<String>,

    // ---- Format / sampler state (consumed by #23). ----
    /// `srgb_framebufferN` (#23). When set the FBO uses an sRGB format
    /// (`Rgba8UnormSrgb`).
    pub srgb_framebuffer: bool,
    /// `float_framebufferN` (#23). When set the FBO uses RGBA16F
    /// (`Rgba16Float`). Wins over `srgb_framebuffer` if both are set (§11.3).
    pub float_framebuffer: bool,
    /// `filter_linearN` (#23). `true`=linear, `false`=nearest input sampling.
    pub filter_linear: bool,
    /// `wrap_modeN` (#23). Sampler wrap mode for this pass's input texture.
    pub wrap_mode: WrapMode,
    /// `mipmap_inputN` (#23). Generate a mip chain for this pass's input.
    pub mipmap_input: bool,

    // ---- Builtin semantics (consumed by #28). ----
    /// `frame_count_modN` (#28). When `> 0`, the `FrameCount` this pass sees is
    /// pre-wrapped: `frame_count % mod` (§6). `0` (the default) means no wrap.
    pub frame_count_mod: u32,
}

impl Pass {
    /// A pass with just a shader and no scale keys (the common case): the chain
    /// builder applies the §2 position default. Reserved fields take #22
    /// defaults (linear filter, no srgb/float/mipmap — §3 v1 choices).
    pub fn new(shader: CompiledShader) -> Self {
        Self {
            shader,
            scale: None,
            alias: None,
            srgb_framebuffer: false,
            float_framebuffer: false,
            filter_linear: true,
            wrap_mode: WrapMode::default(),
            mipmap_input: false,
            frame_count_mod: 0,
        }
    }

    /// Builder: set an explicit scale config.
    pub fn with_scale(mut self, scale: ScaleConfig) -> Self {
        self.scale = Some(scale);
        self
    }

    /// The wgpu render-target format this pass's FBO should use (§3):
    ///
    /// ```text
    /// float_framebuffer -> Rgba16Float       (16-bit float; preserves >1.0/HDR)
    /// srgb_framebuffer  -> Rgba8UnormSrgb     (HW sRGB encode on store/decode on load)
    /// else              -> Rgba8Unorm         (default: 8-bit linear UNORM)
    /// ```
    ///
    /// `float` wins over `srgb` when both are set (§11.3). This applies to an
    /// **intermediate** pass's owned FBO; the final pass always renders into the
    /// 8-bit read-back target (see `renderer::OFFSCREEN_FORMAT`).
    pub fn fbo_format(&self) -> wgpu::TextureFormat {
        if self.float_framebuffer {
            wgpu::TextureFormat::Rgba16Float
        } else if self.srgb_framebuffer {
            wgpu::TextureFormat::Rgba8UnormSrgb
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn axis(ty: ScaleType, factor: f32) -> AxisScale {
        AxisScale { ty, factor }
    }

    #[test]
    fn source_scale_multiplies_input() {
        // source × 2.0 of a 100x50 input -> 200x100, independent of viewport.
        let s = ScaleConfig {
            x: axis(ScaleType::Source, 2.0),
            y: axis(ScaleType::Source, 2.0),
        };
        assert_eq!(s.resolve((100, 50), (640, 480)), (200, 100));
    }

    #[test]
    fn viewport_scale_multiplies_viewport() {
        // viewport × 1.0 of a 640x480 viewport -> 640x480, independent of input.
        let s = ScaleConfig {
            x: axis(ScaleType::Viewport, 1.0),
            y: axis(ScaleType::Viewport, 1.0),
        };
        assert_eq!(s.resolve((100, 50), (640, 480)), (640, 480));
        // viewport × 0.5 -> half the viewport.
        let half = ScaleConfig {
            x: axis(ScaleType::Viewport, 0.5),
            y: axis(ScaleType::Viewport, 0.5),
        };
        assert_eq!(half.resolve((100, 50), (640, 480)), (320, 240));
    }

    #[test]
    fn absolute_scale_is_a_literal_pixel_count() {
        let s = ScaleConfig {
            x: axis(ScaleType::Absolute, 320.0),
            y: axis(ScaleType::Absolute, 240.0),
        };
        // Input and viewport are both ignored.
        assert_eq!(s.resolve((100, 50), (640, 480)), (320, 240));
        assert_eq!(s.resolve((1, 1), (1, 1)), (320, 240));
    }

    #[test]
    fn per_axis_mix_resolves_independently() {
        // absolute X, viewport Y — the per-axis case real presets use.
        let s = ScaleConfig {
            x: axis(ScaleType::Absolute, 256.0),
            y: axis(ScaleType::Viewport, 1.0),
        };
        assert_eq!(s.resolve((100, 50), (800, 600)), (256, 600));
    }

    #[test]
    fn fractional_factors_round_to_nearest() {
        // 100 * 1.5 = 150 (exact); 100 * 0.333 = 33.3 -> 33; 3 * 0.5 = 1.5 -> 2.
        let s = ScaleConfig {
            x: axis(ScaleType::Source, 0.333),
            y: axis(ScaleType::Source, 0.5),
        };
        assert_eq!(s.resolve((100, 3), (0, 0)), (33, 2));
    }

    #[test]
    fn sizes_clamp_to_the_valid_texel_range() {
        // Below 1 clamps up to 1; above 16384 clamps down.
        let tiny = ScaleConfig {
            x: axis(ScaleType::Source, 0.0),
            y: axis(ScaleType::Absolute, 0.0),
        };
        assert_eq!(tiny.resolve((10, 10), (10, 10)), (1, 1));
        let huge = ScaleConfig {
            x: axis(ScaleType::Absolute, 100_000.0),
            y: axis(ScaleType::Source, 1000.0),
        };
        assert_eq!(huge.resolve((100, 100), (0, 0)), (16384, 16384));
    }

    #[test]
    fn axis_default_constants() {
        // source×1 mirrors input; viewport×1 mirrors viewport.
        assert_eq!(AxisScale::SOURCE_1X.resolve(123, 999), 123);
        assert_eq!(AxisScale::VIEWPORT_1X.resolve(123, 999), 999);
    }
}
