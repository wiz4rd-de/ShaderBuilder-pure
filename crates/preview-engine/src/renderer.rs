//! Headless wgpu renderer: an ordered **N-pass** chain that draws a fullscreen
//! quad sampling the source image through each compiled slang pass in turn
//! (Architecture §D/§F; `docs/retroarch-slang-runtime.md` §2/§10). No window —
//! the final pass renders to an offscreen RGBA8 target that is read back.
//!
//! ## Chain model (#22)
//! Pass 0's `Source` is the input [`Frame`] (`Original`); pass `i`'s `Source` is
//! pass `i-1`'s output texture. Intermediate passes render into **owned FBOs**
//! sized by their scale type (§2). The **final pass** renders into the
//! viewport/pane (the offscreen target) directly **when it has no explicit
//! `scale`** (the `viewport × 1.0` default). When the final pass declares an
//! explicit `scaleN`, it instead renders into its OWN scaled FBO (receiving
//! `OutputSize == that FBO size`) and is then **stretched** (a fullscreen-quad
//! blit with a linear sampler) into the viewport-sized offscreen target (§2/§10).
//! FBO sizes are recomputed whenever the viewport or source size changes.
//!
//! ## Back-compat
//! A single `.slang` shader still works as a degenerate 1-pass chain:
//! [`Renderer::set_shader`] wraps it via [`Renderer::set_chain`]. Pass 0 == final
//! pass renders straight to the offscreen target, exactly as the Phase-1 single
//! pass did.
//!
//! SPIR-V from `slang-compile` is ingested via `wgpu::ShaderSource::SpirV` (no
//! WGSL hop). Each pass binds the one-pass set: the builtin uniform buffer
//! (`MVP`, the `*Size` family, `FrameCount` — see [`crate::uniforms`]), the
//! source `texture2D`, a `sampler`, and the parameter UBO. Per-pass `*Size`
//! semantics differ, so each pass owns its own builtin UBO.
//!
//! Note on samplers: wgpu's binding model uses **separate** texture + sampler.
//! A combined GLSL `sampler2D` is **not** yet split into a separate texture +
//! sampler (tracked as a separate task); the current fixtures all use the
//! separate Vulkan `texture2D` + `sampler` form, which is what this renderer binds.

use crate::bindtable::{self, PlaceholderResolver, TextureClass, TextureResolver};
use crate::pass::{AxisScale, Pass, ScaleConfig, WrapMode};
use crate::uniforms::{self, BuiltinValues, ParamStore, ParamView};
use slang_compile::{BlockBinding, CompiledShader, Parameter, SpirvReflection, UniformBlock};
use source::Frame;

/// The offscreen color format. Linear RGBA8 so a passthrough shader reproduces
/// the uploaded image byte-for-byte (no sRGB conversion in the slice).
///
/// The **final pass always renders into this 8-bit linear target** regardless of
/// its `srgb_framebuffer`/`float_framebuffer` keys (#23): the preview reads back
/// 8 bits per channel, so a float/sRGB final FBO would only be re-quantized to
/// RGBA8 on read-back. Per-pass formats apply to **intermediate** FBOs (see
/// [`Pass::fbo_format`]). This matches §3/§11.16: the final/viewport format is
/// the swapchain's (here, the read-back target's).
pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// WGSL for the mipmap-downsample pass (#23). A fullscreen triangle samples the
/// previous mip level with a linear sampler; rendering into mip `k` from mip
/// `k-1` performs a 2×2 box/linear downsample. Used to (re)generate a pass FBO's
/// mip chain each frame when a consumer sets `mipmap_input` (§10 mip timing).
const MIP_WGSL: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;

struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> VsOut {
    // Fullscreen triangle covering the target; uv in [0,1].
    var out: VsOut;
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    out.uv = vec2<f32>(x, y);
    out.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    return textureSampleLevel(src, smp, in.uv, 0.0);
}
"#;

/// Fullscreen-quad vertex: clip-space position + source texture coordinate.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 4],
    uv: [f32; 2],
}

/// Two triangles covering the viewport. Positions are in RetroArch's `[0,1]`
/// quad space — the MVP (an orthographic `[0,1]→[-1,1]` map, see
/// [`uniforms::ortho_mvp`]) projects them to clip space, exactly as a real slang
/// vertex shader's `MVP * Position` does. UVs are set so the image's top-left
/// lands at the framebuffer's top-left (wgpu NDC is y-up; texture row 0 is top).
const QUAD: [Vertex; 6] = [
    Vertex {
        pos: [0.0, 0.0, 0.0, 1.0],
        uv: [0.0, 1.0],
    },
    Vertex {
        pos: [1.0, 0.0, 0.0, 1.0],
        uv: [1.0, 1.0],
    },
    Vertex {
        pos: [1.0, 1.0, 0.0, 1.0],
        uv: [1.0, 0.0],
    },
    Vertex {
        pos: [0.0, 0.0, 0.0, 1.0],
        uv: [0.0, 1.0],
    },
    Vertex {
        pos: [1.0, 1.0, 0.0, 1.0],
        uv: [1.0, 0.0],
    },
    Vertex {
        pos: [0.0, 1.0, 0.0, 1.0],
        uv: [0.0, 0.0],
    },
];

/// Errors initializing or driving the renderer.
#[derive(Debug)]
pub enum RendererError {
    /// No suitable wgpu adapter (no GPU / Vulkan available).
    NoAdapter,
    /// Requesting the device failed.
    Device(wgpu::RequestDeviceError),
    /// `render`/`read_back` called before both a source image and a shader were set.
    NotReady,
    /// A chain was set with zero passes.
    EmptyChain,
    /// Reading back the offscreen target failed.
    Readback(String),
}

impl std::fmt::Display for RendererError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RendererError::NoAdapter => write!(f, "no suitable wgpu adapter found"),
            RendererError::Device(e) => write!(f, "failed to request wgpu device: {e}"),
            RendererError::NotReady => write!(f, "renderer needs both a source image and a shader"),
            RendererError::EmptyChain => write!(f, "a render chain needs at least one pass"),
            RendererError::Readback(e) => write!(f, "offscreen readback failed: {e}"),
        }
    }
}

impl std::error::Error for RendererError {}

/// A reflected texture slot on a pass: the wgpu `binding` number it occupies,
/// and the [`TextureClass`] its GLSL name resolves to (#26). The renderer
/// resolves the class to a live view + the producing pass's sampler each frame in
/// [`Renderer::rebuild_chain`].
struct TextureSlot {
    binding: u32,
    class: TextureClass,
}

/// A reflected sampler slot on a pass: its wgpu `binding` number (#26). The
/// renderer binds the sampler of whichever texture this sampler reads — by §3/§7
/// the *producing* pass's sampler. With one texture+sampler per pass (the common
/// case) the sampler pairs with the pass's single texture slot; the renderer
/// pairs them positionally (first sampler ↔ first texture).
struct SamplerSlot {
    binding: u32,
}

/// GPU resources for one pass in the chain.
struct PassResources {
    pipeline: wgpu::RenderPipeline,
    /// This pass's reflection-driven bind-group layout (#26): built from the
    /// SPIR-V's uniform blocks + textures + samplers at their reflected bindings,
    /// replacing the old single fixed layout. The bind group and pipeline layout
    /// are derived from this.
    bind_group_layout: wgpu::BindGroupLayout,
    /// The reflected texture slots this pass declares, each a `(binding, class)`
    /// (#26). Resolved to live views every frame.
    texture_slots: Vec<TextureSlot>,
    /// The reflected sampler slots this pass declares (#26). Bound to the
    /// producing-pass sampler of the paired texture.
    sampler_slots: Vec<SamplerSlot>,
    /// The reflected uniform-block bindings (set-0 UBOs) this pass declares: the
    /// builtin block's binding and the param block's binding, in reflection order
    /// (#26). Used to attach the right UBO buffer at each block's binding number.
    block_bindings: Vec<u32>,
    /// Parameter UBO (binding 3). Re-packed + re-uploaded each frame from the
    /// chain's global [`ParamStore`] — no recompile/pipeline rebuild (#29).
    param_buffer: wgpu::Buffer,
    /// This pass's builtin UBO (per-pass because `*Size` differs per pass).
    ubo: wgpu::Buffer,
    /// The reflected builtin uniform block this pass declares (#28): member
    /// names → byte offsets, discovered from the SPIR-V so builtins can be
    /// declared in any order/subset. `None` if the pass declares no builtin
    /// block (e.g. a constant-color pass); then the UBO is a zero-filled vec4.
    /// Parameters declared **inside** this block (a mixed builtin+param block, as
    /// real RetroArch shaders use) are packed here too (#29).
    builtin_block: Option<UniformBlock>,
    /// The reflected parameter uniform block (binding 3) this pass declares, if
    /// any: member names → offsets for `#pragma parameter` values that live in a
    /// dedicated params block. `None` when the pass has no such block (the param
    /// UBO is then a zero-filled vec4). #29 packs current param values here.
    param_block: Option<UniformBlock>,
    /// The set-0 binding the **builtin** UBO buffer attaches at (#26): the builtin
    /// block's reflected binding (legacy `0` when absent).
    builtin_binding: u32,
    /// The set-0 binding the **param** UBO buffer attaches at (#26): the param
    /// block's reflected binding (legacy `3` when absent).
    param_binding: u32,
    /// `frame_count_modN` (#28): the `FrameCount` this pass sees is pre-wrapped
    /// by this modulus (`0` = no wrap).
    frame_count_mod: u32,
    /// This pass's **producing** sampler: filter/wrap/mip per the pass's own
    /// `filter_linear`/`wrap_mode`/`mipmap_input` (#23). Per §3/§7 a texture is
    /// sampled with the sampler of the pass that **produced** it, so a *consumer*
    /// reading this pass's output binds *this* sampler — not its own (#26). For a
    /// pass's `Source` input that means pass `i-1`'s sampler; pass 0's `Source`
    /// (and any `Original`) uses [`Renderer::original_sampler`]. Built once at
    /// [`Renderer::build_pass`].
    sampler: wgpu::Sampler,
    /// The render-target format this pass writes (its FBO format for an
    /// intermediate pass; [`OFFSCREEN_FORMAT`] for the final pass). The pipeline
    /// was built for this format, so the FBO must match it.
    target_format: wgpu::TextureFormat,
    /// Explicit scale config, or `None` to take the §2 position default.
    scale: Option<ScaleConfig>,
    /// `true` if this pass owns an FBO (sized by its scale) that it renders into:
    /// every **intermediate** pass, plus a **final** pass that declares an
    /// explicit `scale` (#22). A final pass with no explicit scale owns no FBO and
    /// renders straight into the shared offscreen target.
    owns_fbo: bool,
    /// `true` only for a **final** pass that owns an FBO (explicit `scaleN`): after
    /// it draws into its scaled FBO, the FBO is stretched (fullscreen-quad blit
    /// with a linear sampler) into the viewport-sized offscreen target (#22 §2/§10).
    final_owns_fbo: bool,
    /// `true` if a **downstream** consumer reads this pass's output with
    /// `mipmap_input` (#23): its FBO must carry a full mip chain that we
    /// regenerate each frame right after this pass draws (§10 mip timing).
    produces_mips: bool,
    /// `true` if **this** pass reads its own input with `mipmap_input` (#23): its
    /// input texture must carry a mip chain. For pass 0 this drives the source
    /// texture's mip allocation (#23/F); for later passes it is already reflected
    /// in the previous pass's `produces_mips`.
    consumes_input_mips: bool,
    /// For an intermediate pass: its owned FBO, allocated on the first render and
    /// reallocated when its size/format/mip-count changes. `None` for the final
    /// pass, and for an intermediate pass before its first allocation.
    fbo: Option<Fbo>,
    /// The bind group for this pass, rebuilt when its input texture changes.
    bind_group: Option<wgpu::BindGroup>,
}

/// An owned intermediate render target. Holds the `view` (sampled by the next
/// pass) and the `texture` (needed to create per-mip-level views for mip
/// generation), plus the size/format/mip-count it was allocated at so we can
/// detect when a viewport/source/format change needs a realloc.
struct Fbo {
    texture: wgpu::Texture,
    /// Full view spanning all mip levels — what the **next** pass samples (so a
    /// `mipmap_input` consumer can read coarse mips).
    view: wgpu::TextureView,
    /// Single base-level (mip 0) view — what **this** pass renders into (a color
    /// attachment must target exactly one mip level).
    base_view: wgpu::TextureView,
    size: (u32, u32),
    format: wgpu::TextureFormat,
    mip_level_count: u32,
}

/// A headless N-pass renderer.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    vertex_buffer: wgpu::Buffer,
    /// A default source-defaults sampler (linear, clamp-to-border, no mips) used
    /// only as a **fallback** when the chain is empty (#26). Normally the source /
    /// `Original` is sampled with pass 0's sampler (the "K+1" rule with the source
    /// produced by "pass -1"); see [`Renderer::sampler_after_source`].
    original_sampler: wgpu::Sampler,
    /// The chain's alias table: each pass's `alias` (its `#pragma name`/preset
    /// `aliasN`) mapped to its pass index (#26). A `<alias>` texture binds the
    /// output of `aliases[alias]`. Rebuilt by `set_chain`.
    aliases: std::collections::HashMap<String, usize>,
    /// The deferred-resource resolver (#26): the hook #24 (feedback) / #25
    /// (history) / #27 (LUTs) implement. Defaults to a [`PlaceholderResolver`]
    /// returning a 1×1 black texture so unimplemented semantics still bind.
    resolver: PlaceholderResolver,
    /// Whether the device supports `AddressMode::ClampToBorder` (#23). When
    /// `false`, a pass asking for `WrapMode::ClampToBorder` falls back to
    /// `ClampToEdge` (so lavapipe/CI without the feature still works).
    clamp_to_border_supported: bool,
    /// Lazily-built resources for mipmap generation (pipeline + sampler), shared
    /// across passes and formats keyed by target format.
    mip_gen: Option<MipGen>,
    /// Lazily-built resources for the final-pass stretch blit (#22): used only
    /// when the last pass declares an explicit `scale` and renders into its own
    /// FBO that must then be stretched into the offscreen target.
    blit: Option<Blit>,

    width: u32,
    height: u32,
    offscreen: wgpu::Texture,

    /// Size of the uploaded source image, for the `*Size` uniforms.
    source_size: Option<(u32, u32)>,
    /// Frames rendered so far; written into `FrameCount` and bumped per frame.
    frame_count: u32,
    /// `FrameDirection` (#28): `+1` forward, `-1` rewinding. Settable so rewind
    /// (#31) can flip it; `+1` for now.
    frame_direction: i32,

    /// The uploaded source texture as an [`Fbo`] (so it can carry a mip chain when
    /// pass 0 declares `mipmap_input` — #23/F). Its `view` (all mips) is pass 0's
    /// input. Reallocated with a full mip count + RENDER_ATTACHMENT when pass 0
    /// wants mips; allocated with one level otherwise.
    source: Option<Fbo>,
    /// The source image's raw RGBA, retained so the source texture can be
    /// reallocated (e.g. to add a mip chain for `mipmap_input0`) and re-uploaded
    /// without the caller re-supplying the frame (#23/F).
    source_rgba: Option<Vec<u8>>,
    /// The ordered pass chain. Empty until [`Renderer::set_shader`]/`set_chain`.
    passes: Vec<PassResources>,

    /// The chain's **global-by-name** parameter state (#29): every pass's
    /// `#pragma parameter`s deduped by name, with live `current` values. Rebuilt
    /// by `set_chain`; mutated by [`Renderer::set_parameter`] (no recompile — the
    /// next frame just re-packs + re-uploads the param UBOs).
    params: ParamStore,
}

/// Resources for the per-frame mipmap-downsample blit (#23): the shader module,
/// a dedicated bind-group layout (texture + linear sampler), the linear sampler,
/// and a per-target-format pipeline cache (a render pipeline is format-specific).
struct MipGen {
    module: wgpu::ShaderModule,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    sampler: wgpu::Sampler,
    pipelines: std::collections::HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
}

/// Resources for the final-pass **stretch blit** (#22): a fullscreen-quad
/// passthrough (reusing [`MIP_WGSL`], which samples its source at LOD 0) that
/// reads a final pass's own scaled FBO and stretches it into the viewport-sized
/// offscreen target with a **linear** sampler. The pipeline targets
/// [`OFFSCREEN_FORMAT`] (the offscreen target's format), so a single cached
/// pipeline suffices.
struct Blit {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pipeline: wgpu::RenderPipeline,
}

/// Fallback builtin-UBO size when a pass declares no builtin block (#28): one
/// std140 vec4 of zero storage, since a bound UBO needs at least one vec4 even
/// when unused (mirrors `pack_parameters`' minimum). When a builtin block *is*
/// reflected, the UBO is sized to the reflected block instead.
const UBO_FALLBACK_SIZE: u64 = 16;

impl Renderer {
    /// Initialize a headless wgpu device and the static resources.
    pub fn new(width: u32, height: u32) -> Result<Self, RendererError> {
        let width = width.max(1);
        let height = height.max(1);

        // Honor WGPU_BACKEND etc. from the environment (lets CI force a software
        // Vulkan/GL adapter via lavapipe/llvmpipe).
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .map_err(|_| RendererError::NoAdapter)?;

        // `WrapMode::ClampToBorder` (the RetroArch default, §3) needs an optional
        // wgpu feature. Request it ONLY if the adapter advertises it, so software
        // adapters (lavapipe/llvmpipe on CI) that lack it still get a device; a
        // pass asking for ClampToBorder then falls back to ClampToEdge.
        let clamp_to_border_supported = adapter
            .features()
            .contains(wgpu::Features::ADDRESS_MODE_CLAMP_TO_BORDER);
        let required_features = if clamp_to_border_supported {
            wgpu::Features::ADDRESS_MODE_CLAMP_TO_BORDER
        } else {
            wgpu::Features::empty()
        };

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("preview-engine device"),
            required_features,
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::Off,
        }))
        .map_err(RendererError::Device)?;

        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fullscreen quad"),
            size: std::mem::size_of_val(&QUAD) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&vertex_buffer, 0, bytemuck::cast_slice(&QUAD));

        // The sampler for `Original` (and pass 0's `Source == Original`): §3 v1
        // source defaults — linear filter, clamp-to-border wrap, no mips (#26).
        let original_address = if clamp_to_border_supported {
            wgpu::AddressMode::ClampToBorder
        } else {
            wgpu::AddressMode::ClampToEdge
        };
        let original_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Original sampler"),
            address_mode_u: original_address,
            address_mode_v: original_address,
            address_mode_w: original_address,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            lod_min_clamp: 0.0,
            lod_max_clamp: 0.0,
            border_color: Some(wgpu::SamplerBorderColor::TransparentBlack),
            ..Default::default()
        });

        let resolver = PlaceholderResolver::new(&device, &queue);

        let offscreen = create_offscreen(&device, width, height);

        Ok(Self {
            device,
            queue,
            vertex_buffer,
            original_sampler,
            aliases: std::collections::HashMap::new(),
            resolver,
            clamp_to_border_supported,
            mip_gen: None,
            blit: None,
            width,
            height,
            offscreen,
            source_size: None,
            frame_count: 0,
            frame_direction: 1,
            source: None,
            source_rgba: None,
            passes: Vec::new(),
            params: ParamStore::default(),
        })
    }

    /// Whether this device supports `AddressMode::ClampToBorder` (#23). When
    /// `false`, `WrapMode::ClampToBorder` falls back to `ClampToEdge`.
    pub fn clamp_to_border_supported(&self) -> bool {
        self.clamp_to_border_supported
    }

    /// The current offscreen target (final-viewport) size.
    pub fn viewport(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Resize the offscreen / final-viewport target. Intermediate FBO sizes are
    /// recomputed lazily on the next [`Renderer::render`] (a `viewport`-scaled
    /// pass reallocates when the viewport changes, §2).
    pub fn set_viewport(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if (width, height) != (self.width, self.height) {
            self.width = width;
            self.height = height;
            self.offscreen = create_offscreen(&self.device, width, height);
        }
    }

    /// The source texture's format. Linear RGBA8 — a passthrough shader
    /// reproduces the uploaded image byte-for-byte.
    const SOURCE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

    /// Upload a source image. The raw RGBA is retained so the source texture can
    /// later be reallocated with a mip chain when pass 0 declares `mipmap_input`
    /// (#23/F). The texture starts single-level; [`Renderer::ensure_source_mips`]
    /// upgrades it on the next render if pass 0 needs mips.
    pub fn set_source(&mut self, frame: &Frame) {
        self.source_size = Some((frame.width, frame.height));
        self.source_rgba = Some(frame.rgba.clone());
        self.source = Some(self.build_source((frame.width, frame.height), &frame.rgba, 1));
    }

    /// Build the source texture as an [`Fbo`] of `size` with `mip_level_count`
    /// levels and upload `rgba` into the base level (#23/F). When `mip_level_count
    /// > 1` the texture also gets RENDER_ATTACHMENT usage so the mip-gen blit can
    /// render the coarser levels. Coarser levels are filled by `generate_mips`.
    fn build_source(&self, size: (u32, u32), rgba: &[u8], mip_level_count: u32) -> Fbo {
        let extent = wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        };
        let mut usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
        if mip_level_count > 1 {
            usage |= wgpu::TextureUsages::RENDER_ATTACHMENT;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("source image"),
            size: extent,
            mip_level_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: Self::SOURCE_FORMAT,
            usage,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size.0 * 4),
                rows_per_image: Some(size.1),
            },
            extent,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let base_view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("source base level"),
            base_mip_level: 0,
            mip_level_count: Some(1),
            ..Default::default()
        });
        Fbo {
            texture,
            view,
            base_view,
            size,
            format: Self::SOURCE_FORMAT,
            mip_level_count,
        }
    }

    /// Set a single shader as a degenerate **1-pass chain** (back-compat with the
    /// Phase-1 single-pass API). The pass renders straight to the offscreen
    /// target. Equivalent to `set_chain(&[Pass::new(shader.clone())])`.
    pub fn set_shader(&mut self, shader: &CompiledShader) {
        // Infallible for a 1-pass chain (only `EmptyChain` can fail).
        let _ = self.set_chain(std::slice::from_ref(&Pass::new(shader.clone())));
    }

    /// Set an ordered N-pass chain. Builds each pass's pipeline, parameter UBO,
    /// builtin UBO, and (for intermediate passes) an owned FBO. The final pass
    /// renders into the offscreen target. Returns [`RendererError::EmptyChain`]
    /// if `passes` is empty.
    pub fn set_chain(&mut self, passes: &[Pass]) -> Result<(), RendererError> {
        if passes.is_empty() {
            return Err(RendererError::EmptyChain);
        }
        // The chain's alias table (#26): each pass's `alias` → its index, so a
        // later pass sampling `<alias>` (or reading `<alias>Size`) resolves to the
        // aliased pass's output. Built before the passes so classification can use
        // it. A duplicate alias keeps the first (lowest-index) pass.
        let mut aliases = std::collections::HashMap::new();
        for (i, pass) in passes.iter().enumerate() {
            if let Some(name) = &pass.alias {
                aliases.entry(name.clone()).or_insert(i);
            }
        }
        let alias_names: Vec<String> = aliases.keys().cloned().collect();
        self.aliases = aliases;

        let last = passes.len() - 1;
        let mut resources = Vec::with_capacity(passes.len());
        for (i, pass) in passes.iter().enumerate() {
            let is_final = i == last;
            // Pass `i`'s output is sampled with mips iff the *next* pass reads it
            // with `mipmap_input` (its input is this pass's FBO). The final pass
            // never produces mips (nothing reads it as Source).
            let produces_mips = !is_final && passes[i + 1].mipmap_input;
            resources.push(self.build_pass(pass, is_final, produces_mips, &alias_names));
        }
        self.passes = resources;

        // Collect the chain's parameters (global by name, deduped — §8), seeded
        // to each `#pragma parameter` default. Aliases come from each pass's
        // `#pragma name` if it coincides with a parameter name; presets may also
        // carry an explicit alias (#22 `aliasN`), wired by the app via
        // [`Renderer::set_parameter_alias`] before/with overrides if needed.
        let param_lists: Vec<&[Parameter]> = passes
            .iter()
            .map(|p| p.shader.reflection.parameters.as_slice())
            .collect();
        let aliases = std::collections::HashMap::new();
        self.params = ParamStore::collect(param_lists, &aliases);

        // FBO sizes + bind groups are (re)built lazily at render time, once the
        // source size is known and the full chain exists.
        Ok(())
    }

    /// Replace the chain's parameter state wholesale (#29) — used by the app to
    /// apply a preset's `parameter_overrides` (and any aliases) after building the
    /// chain. The current values take effect on the next frame's param packing; no
    /// recompile or pipeline rebuild.
    pub fn set_params(&mut self, params: ParamStore) {
        self.params = params;
    }

    /// The chain's collected parameters as a fresh [`ParamStore`] (defaults
    /// seeded, no overrides) — the basis the app layers preset overrides onto.
    pub fn collected_params(&self) -> &ParamStore {
        &self.params
    }

    /// Set a parameter's current value live by canonical name or alias, clamped to
    /// its `[min, max]` range (#29). Returns `true` if the parameter exists. This
    /// performs **no** shader recompile or pipeline rebuild: the next
    /// [`Renderer::render`] re-packs the updated value into each pass's param UBO
    /// and re-uploads it. A name matching no parameter is a no-op (`false`).
    pub fn set_parameter(&mut self, name: &str, value: f32) -> bool {
        self.params.set(name, value)
    }

    /// The current parameter set (name/label/min/max/step/value) in declaration
    /// order, for a data-driven slider UI (#29).
    pub fn parameters(&self) -> Vec<ParamView> {
        self.params.views()
    }

    /// Build the GPU resources for one pass: pipeline, parameter UBO (from the
    /// shader's reflected `#pragma parameter` defaults), the per-pass builtin
    /// UBO, and — for an intermediate pass — a placeholder FBO slot (sized on the
    /// first render).
    fn build_pass(
        &self,
        pass: &Pass,
        is_final: bool,
        produces_mips: bool,
        alias_names: &[String],
    ) -> PassResources {
        let shader = &pass.shader;

        // A final pass with an EXPLICIT scale owns its own FBO (sized by that
        // scale) and is then stretched into the viewport-sized offscreen target
        // (#22, §2/§10). A final pass with no explicit scale keeps the direct
        // `viewport × 1.0` default: it renders straight into the offscreen target.
        let final_owns_fbo = is_final && pass.scale.is_some();
        let owns_fbo = !is_final || final_owns_fbo;

        // The format this pass writes: a pass that owns an FBO (any intermediate,
        // or a final pass with an explicit scale) writes its selected per-pass
        // format (§3); a final pass with no FBO writes the 8-bit read-back target
        // (OFFSCREEN_FORMAT). The pipeline's color target MUST match the FBO it
        // renders into, so build the pipeline for this format.
        let target_format = if owns_fbo {
            pass.fbo_format()
        } else {
            OFFSCREEN_FORMAT
        };

        // This pass's input sampler (binding 2): filter + wrap + mip per its
        // `filter_linear`/`wrap_mode`/`mipmap_input` (#23). `mipmap_input` raises
        // `lod_max_clamp` and selects a linear mipmap filter so coarse mips are
        // sampled; otherwise lod is clamped to the base level.
        let sampler = self.build_sampler(pass);

        // Reflect the SPIR-V once to discover both blocks' member offsets (#28/
        // #29) so builtins/params can be declared in any order/subset. A
        // reflection failure is non-fatal: fall back to no blocks (zero-filled
        // UBOs) rather than refusing to build the pass — but log it loudly so the
        // degradation (a zero MVP renders nothing) is not silent (#28).
        let reflection = match slang_compile::reflect(shader) {
            Ok(r) => Some(r),
            Err(e) => {
                let name = shader.reflection.name.as_deref().unwrap_or("<unnamed>");
                eprintln!(
                    "preview-engine: SPIR-V reflection failed for shader {name:?}: {e} \
                     — packing builtins/params with no reflected layout (the pass \
                     may render incorrectly)"
                );
                None
            }
        };
        // The reflection-driven bind layout + texture/sampler slots (#26). When
        // reflection failed we fall back to an empty reflection: the legacy
        // fixtures all reflect cleanly, and a failed reflection already renders
        // incorrectly (logged above), so an empty layout is acceptable degradation.
        let empty = SpirvReflection::default();
        let refl = reflection.as_ref().unwrap_or(&empty);
        let bind_group_layout = bindtable::pass_layout(&self.device, refl);
        let texture_slots: Vec<TextureSlot> = refl
            .textures
            .iter()
            .filter(|t| t.set == 0)
            .map(|t| TextureSlot {
                binding: t.binding,
                class: TextureClass::classify(&t.name, alias_names),
            })
            .collect();
        let sampler_slots: Vec<SamplerSlot> = refl
            .samplers
            .iter()
            .filter(|s| s.set == 0)
            .map(|s| SamplerSlot { binding: s.binding })
            .collect();
        let block_bindings: Vec<u32> = refl
            .blocks
            .iter()
            .filter_map(|b| match b.binding {
                BlockBinding::Uniform { set: 0, binding } => Some(binding),
                _ => None,
            })
            .collect();

        let builtin_block = reflection
            .as_ref()
            .and_then(|r| uniforms::builtin_block(r).cloned());
        // The dedicated parameter block: any reflected block that is NOT the
        // builtin block (#29). Parameters declared *inside* the builtin block are
        // packed there directly (a mixed block); this catches the common separate
        // `Params` UBO at binding 3.
        let param_block = reflection.as_ref().and_then(|r| {
            r.blocks
                .iter()
                .find(|b| Some(b.binding) != builtin_block.as_ref().map(|bb| bb.binding))
                .cloned()
        });
        // Size the param UBO to the reflected block (a 16-byte multiple) or one
        // vec4 when there is none — a bound UBO needs at least one vec4 of
        // storage even when the shader declares no parameters. The buffer is
        // written each frame from the chain's ParamStore (#29).
        let param_size = param_block
            .as_ref()
            .map(|b| b.size as u64)
            .unwrap_or(UBO_FALLBACK_SIZE)
            .max(UBO_FALLBACK_SIZE);
        let param_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("parameters"),
            size: param_size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Size the builtin UBO to the reflected block (a 16-byte multiple), or
        // to one vec4 when there is no builtin block.
        let ubo_size = builtin_block
            .as_ref()
            .map(|b| b.size as u64)
            .unwrap_or(UBO_FALLBACK_SIZE)
            .max(UBO_FALLBACK_SIZE);
        let ubo = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniforms"),
            size: ubo_size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let vs = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("vertex"),
                source: wgpu::ShaderSource::SpirV(std::borrow::Cow::Borrowed(&shader.vertex_spirv)),
            });
        let fs = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("fragment"),
                source: wgpu::ShaderSource::SpirV(std::borrow::Cow::Borrowed(
                    &shader.fragment_spirv,
                )),
            });

        let layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("pass pipeline layout"),
                bind_group_layouts: &[Some(&bind_group_layout)],
                immediate_size: 0,
            });

        // The color target's format matches the FBO this pass renders into
        // (#23): per-pass float/sRGB/default for an intermediate, OFFSCREEN_FORMAT
        // for the final pass. A pipeline is format-specific, so this is baked in.
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("chain pass"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &vs,
                    entry_point: Some("main"),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 0,
                                shader_location: 0,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 16,
                                shader_location: 1,
                            },
                        ],
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &fs,
                    entry_point: Some("main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target_format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });

        // The binding number each UBO buffer attaches at (#26): the builtin block's
        // and the param block's reflected `binding` within set 0. Default to the
        // legacy 0 / 3 if a block is absent (the buffer is then a zero-filled vec4
        // bound at a binding the layout may not list — harmless, but normally the
        // layout carries it because the shader declares the block).
        let builtin_binding = block_binding(builtin_block.as_ref()).unwrap_or(0);
        let param_binding = block_binding(param_block.as_ref()).unwrap_or(3);

        PassResources {
            pipeline,
            bind_group_layout,
            texture_slots,
            sampler_slots,
            block_bindings,
            param_buffer,
            ubo,
            builtin_block,
            param_block,
            builtin_binding,
            param_binding,
            frame_count_mod: pass.frame_count_mod,
            sampler,
            target_format,
            scale: pass.scale,
            owns_fbo,
            final_owns_fbo,
            produces_mips,
            consumes_input_mips: pass.mipmap_input,
            fbo: None,
            bind_group: None,
        }
    }

    /// Build a pass's input sampler from its `filter_linear`/`wrap_mode`/
    /// `mipmap_input` (#23). Address modes use [`Renderer::address_mode`] (which
    /// applies the ClampToBorder→ClampToEdge fallback). The mipmap filter follows
    /// `filter_linear` (librashader's `mip_filter: filter`): `Linear` only when
    /// both `mipmap_input` and `filter_linear` are set, else `Nearest`.
    /// `lod_max_clamp` is left at its default (`f32::MAX`) when `mipmap_input` is
    /// set so coarse mips can be sampled; otherwise lod is clamped to the base
    /// level so only mip 0 is read.
    fn build_sampler(&self, pass: &Pass) -> wgpu::Sampler {
        let filter = if pass.filter_linear {
            wgpu::FilterMode::Linear
        } else {
            wgpu::FilterMode::Nearest
        };
        let address = self.address_mode(pass.wrap_mode);
        let (mipmap_filter, lod_max_clamp) = if pass.mipmap_input {
            // Mips consumed: allow the full chain. The mip filter tracks
            // `filter_linear` (librashader `mip_filter: filter`), so a nearest
            // pass also samples mips with nearest.
            let mip_filter = if pass.filter_linear {
                wgpu::MipmapFilterMode::Linear
            } else {
                wgpu::MipmapFilterMode::Nearest
            };
            (mip_filter, f32::MAX)
        } else {
            // No mips consumed: pin to the base level.
            (wgpu::MipmapFilterMode::Nearest, 0.0)
        };
        self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("pass input sampler"),
            address_mode_u: address,
            address_mode_v: address,
            address_mode_w: address,
            mag_filter: filter,
            min_filter: filter,
            mipmap_filter,
            lod_min_clamp: 0.0,
            lod_max_clamp,
            // A transparent-black border matches RetroArch's clamp_to_border
            // (used only when the device supports ClampToBorder).
            border_color: Some(wgpu::SamplerBorderColor::TransparentBlack),
            ..Default::default()
        })
    }

    /// Map a [`WrapMode`] to a wgpu [`AddressMode`] (#23). `ClampToBorder` is the
    /// RetroArch default but needs `ADDRESS_MODE_CLAMP_TO_BORDER`; when the device
    /// lacks it we fall back to `ClampToEdge` (RetroArch's border is
    /// transparent-black-ish, so clamping to the edge is the closest baseline
    /// choice — documented fallback, keeps CI/lavapipe working).
    fn address_mode(&self, wrap: WrapMode) -> wgpu::AddressMode {
        match wrap {
            WrapMode::Repeat => wgpu::AddressMode::Repeat,
            WrapMode::MirroredRepeat => wgpu::AddressMode::MirrorRepeat,
            WrapMode::ClampToEdge => wgpu::AddressMode::ClampToEdge,
            WrapMode::ClampToBorder => {
                if self.clamp_to_border_supported {
                    wgpu::AddressMode::ClampToBorder
                } else {
                    wgpu::AddressMode::ClampToEdge
                }
            }
        }
    }

    /// The effective scale for an intermediate pass: its explicit config, else
    /// the §2 default `source × 1.0` (FBO matches its input).
    fn intermediate_scale(scale: Option<ScaleConfig>) -> ScaleConfig {
        scale.unwrap_or(ScaleConfig {
            x: AxisScale::SOURCE_1X,
            y: AxisScale::SOURCE_1X,
        })
    }

    /// Whether pass 0 reads its input (the source) with `mipmap_input` (#23/F).
    fn pass0_wants_source_mips(&self) -> bool {
        self.passes.first().is_some_and(|p| p.consumes_input_mips)
    }

    /// Upgrade (or downgrade) the source texture's mip chain to match pass 0's
    /// `mipmap_input` (#23/F). When pass 0 wants mips and the source is currently
    /// single-level, reallocate it with a full mip count + RENDER_ATTACHMENT and
    /// re-upload the retained base image; the coarse levels are filled by
    /// `generate_mips` each frame (before pass 0 draws). A no-op when the source's
    /// current mip count already matches. Must run **before** `rebuild_chain` so
    /// pass 0's bind group points at the (possibly new) source view.
    fn ensure_source_mips(&mut self) {
        let (Some(size), Some(rgba)) = (self.source_size, self.source_rgba.as_ref()) else {
            return;
        };
        let want = if self.pass0_wants_source_mips() {
            mip_level_count_for(size)
        } else {
            1
        };
        let have = self.source.as_ref().map(|s| s.mip_level_count).unwrap_or(0);
        if want != have {
            let rgba = rgba.clone();
            self.source = Some(self.build_source(size, &rgba, want));
        }
    }

    /// (Re)allocate intermediate FBOs to the sizes the current source + viewport
    /// imply, and wire each pass's bind group to its input texture (pass 0 ←
    /// source; pass i ← pass i-1's FBO). Called at the top of [`render`].
    fn rebuild_chain(&mut self) {
        let Some(source_view) = self.source.as_ref().map(|s| &s.view) else {
            return;
        };
        let source_size = self.source_size.unwrap_or((self.width, self.height));
        let viewport = (self.width, self.height);

        // Pass 1: resolve + (re)allocate every owned FBO, tracking the running
        // input size down the chain (§2: a `source` scale on pass n is relative to
        // FBO n-1, not to Original). A final pass with an explicit scale also owns
        // an FBO here (#22); it is later stretched into the offscreen target.
        let mut input_size = source_size;
        for res in &mut self.passes {
            if res.owns_fbo {
                let scale = Self::intermediate_scale(res.scale);
                let size = scale.resolve(input_size, viewport);
                // An FBO read downstream with `mipmap_input` carries a full mip
                // chain; otherwise just the base level (#23). Reallocate when
                // size, format, or mip-count changes.
                let mip_level_count = if res.produces_mips {
                    mip_level_count_for(size)
                } else {
                    1
                };
                let stale = match res.fbo.as_ref() {
                    Some(f) => {
                        f.size != size
                            || f.format != res.target_format
                            || f.mip_level_count != mip_level_count
                    }
                    None => true,
                };
                if stale {
                    res.fbo = Some(Fbo::allocate(
                        &self.device,
                        size,
                        res.target_format,
                        mip_level_count,
                    ));
                }
                input_size = size;
            } else {
                // Final pass with no FBO: output is the viewport.
                input_size = viewport;
            }
        }

        // Pass 2: build each pass's reflection-driven bind group (#26). For every
        // reflected texture the pass declares we resolve its [`TextureClass`] to a
        // live view + the *producing* pass's sampler (§3/§7), then assemble the
        // bind group with the UBO/param buffers at their reflected bindings,
        // textures at theirs, and samplers paired positionally to the textures.
        //
        // Bind groups for all passes are built into one Vec first (borrowing
        // `&self.passes` immutably so a pass can read *other* passes' FBO views for
        // `PassOutputN`/`<alias>`), then assigned back in a second loop.
        let count = self.passes.len();
        let mut new_groups: Vec<wgpu::BindGroup> = Vec::with_capacity(count);
        for i in 0..count {
            let res = &self.passes[i];
            let mut entries: Vec<wgpu::BindGroupEntry> = Vec::new();

            // Uniform blocks at their reflected bindings: the builtin UBO and the
            // param UBO. (A block the shader doesn't declare isn't in the layout,
            // so we only attach buffers for bindings the layout lists.)
            for &binding in &res.block_bindings {
                let buffer = if binding == res.param_binding && binding != res.builtin_binding {
                    &res.param_buffer
                } else {
                    &res.ubo
                };
                entries.push(wgpu::BindGroupEntry {
                    binding,
                    resource: buffer.as_entire_binding(),
                });
            }

            // Resolve each texture slot to (view, producing sampler). The sampler
            // is paired to the texture by position (slot j ↔ texture j).
            let mut resolved_samplers: Vec<&wgpu::Sampler> =
                Vec::with_capacity(res.texture_slots.len());
            for slot in &res.texture_slots {
                let (view, sampler) = self.resolve_texture(&slot.class, i, source_view);
                entries.push(wgpu::BindGroupEntry {
                    binding: slot.binding,
                    resource: wgpu::BindingResource::TextureView(view),
                });
                resolved_samplers.push(sampler);
            }
            for (j, slot) in res.sampler_slots.iter().enumerate() {
                // Pair sampler j with texture j's producing sampler; if a pass has
                // more samplers than textures, fall back to the Original sampler.
                let sampler = resolved_samplers
                    .get(j)
                    .copied()
                    .unwrap_or(&self.original_sampler);
                entries.push(wgpu::BindGroupEntry {
                    binding: slot.binding,
                    resource: wgpu::BindingResource::Sampler(sampler),
                });
            }

            new_groups.push(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("pass bind group (reflection-driven)"),
                layout: &res.bind_group_layout,
                entries: &entries,
            }));
        }
        for (i, group) in new_groups.into_iter().enumerate() {
            self.passes[i].bind_group = Some(group);
        }
    }

    /// Resolve a reflected texture's [`TextureClass`] to its live `(view,
    /// sampler)` for consuming pass `i` (#26, §3/§7).
    ///
    /// **Sampler attribution — the "producer's-successor" (K+1) rule.** RetroArch's
    /// real behavior (libretro/RetroArch#14437): a texture produced by pass `K` is
    /// sampled with the `filter_linear`/`wrap_mode`/`mipmap_input` of pass `K+1`
    /// (the pass *immediately after* its producer), **not** the producer itself and
    /// not the (possibly much later) consumer. This reconciles #23: a pass's
    /// `Source` is its predecessor's output (`K = i-1`), so it is sampled with pass
    /// `(i-1)+1 = i`'s config — the consuming pass itself, exactly what #23 built
    /// per-pass. `Original`/pass-0 `Source` is "produced by pass -1", so it is
    /// sampled with pass 0's config. We implement this by selecting
    /// [`Self::sampler_after`]`(K)` = the sampler of pass `K+1` (clamped to the
    /// last pass), where `K` is the producing pass index (`-1` → pass 0).
    ///
    /// Texture mapping (§7): `Source` → pass `i-1`'s output (pass 0: the source
    /// image); `Original` → the source image (any pass); `PassOutputN`/`<alias>` →
    /// pass `N`'s output (causal `N < i`); deferred classes
    /// (`PassFeedbackN`/`OriginalHistoryN`/LUT) → the resolver hook (a 1×1 black
    /// placeholder today). A non-causal / unsatisfiable resource falls back to the
    /// placeholder so the bind never fails.
    fn resolve_texture<'a>(
        &'a self,
        class: &TextureClass,
        pass_index: usize,
        source_view: &'a wgpu::TextureView,
    ) -> (&'a wgpu::TextureView, &'a wgpu::Sampler) {
        // A pass's output FBO view, if it owns one and is causal (earlier than the
        // consumer). The final pass owns no FBO, so reading it as PassOutput is
        // unsatisfiable. Paired with the producer's-successor sampler (K+1 rule).
        let pass_output = |n: usize| -> Option<(&wgpu::TextureView, &wgpu::Sampler)> {
            if n >= pass_index {
                return None; // causal: only earlier passes are available this frame
            }
            let view = &self.passes.get(n)?.fbo.as_ref()?.view;
            Some((view, self.sampler_after(n)))
        };
        match class {
            // Pass 0's Source IS the source image, produced by "pass -1" → sampled
            // with pass 0's sampler. Pass i's Source is pass i-1's output → pass
            // (i-1)+1 = i's sampler (the consuming pass, matching #23).
            TextureClass::Source => {
                if pass_index == 0 {
                    (source_view, self.sampler_after_source())
                } else {
                    pass_output(pass_index - 1)
                        .unwrap_or((source_view, self.sampler_after_source()))
                }
            }
            // Original is the source image (produced by "pass -1") → pass 0's
            // sampler, for any consuming pass.
            TextureClass::Original => (source_view, self.sampler_after_source()),
            TextureClass::PassOutput(n) => {
                pass_output(*n).unwrap_or_else(|| self.placeholder_resource())
            }
            TextureClass::Alias(name) => self
                .aliases
                .get(name)
                .and_then(|&n| pass_output(n))
                .unwrap_or_else(|| self.placeholder_resource()),
            // Deferred resources (#24/#25/#27): the resolver hook returns a
            // placeholder black view today. Sampled with the source sampler.
            other => {
                let view = self
                    .resolver
                    .resolve(other, pass_index)
                    .unwrap_or_else(|| self.resolver.black());
                (view, self.sampler_after_source())
            }
        }
    }

    /// The sampler for a texture produced by pass `producer` — pass `producer+1`'s
    /// sampler (the "K+1" rule, §3/§7 / RetroArch#14437), clamped to the last
    /// pass so a final-pass producer reuses its own sampler.
    fn sampler_after(&self, producer: usize) -> &wgpu::Sampler {
        let idx = (producer + 1).min(self.passes.len().saturating_sub(1));
        &self.passes[idx].sampler
    }

    /// The sampler for the source image / `Original` ("produced by pass -1") —
    /// pass 0's sampler (the K+1 rule with K = -1). Falls back to the dedicated
    /// [`Renderer::original_sampler`] only if the chain is somehow empty.
    fn sampler_after_source(&self) -> &wgpu::Sampler {
        self.passes
            .first()
            .map(|p| &p.sampler)
            .unwrap_or(&self.original_sampler)
    }

    /// The placeholder `(view, sampler)` for an unsatisfiable renderer-resolved
    /// texture (a non-causal `PassOutputN`, an unknown alias): a 1×1 black view +
    /// the source sampler, so the bind succeeds with a defined value.
    fn placeholder_resource(&self) -> (&wgpu::TextureView, &wgpu::Sampler) {
        (self.resolver.black(), self.sampler_after_source())
    }

    /// Compute and upload each pass's builtin UBO for the current frame (#28),
    /// reflection-driven: for each pass we compute every currently-computable
    /// builtin semantic, then pack each into the offset of the same-named member
    /// the pass's SPIR-V actually declares (so order/subset are free). A pass's
    /// `Source`/`Original`/`Output` sizes follow the §2 chaining: pass 0's input
    /// is the source image (== `Original`); pass i's input is FBO i-1; the output
    /// is the pass's own target size. `FinalViewportSize` is the pane; each pass
    /// also sees the output sizes of all *earlier* passes via `PassKSize` /
    /// `PassOutputKSize` (§7).
    fn write_uniforms(&self) {
        let original = self.source_size.unwrap_or((self.width, self.height));
        let viewport = (self.width, self.height);
        let mut input_size = original;
        // Earlier passes' output sizes, grown as we walk the chain so pass i sees
        // passes 0..i (causal — §7).
        let mut pass_output_sizes: Vec<[f32; 4]> = Vec::with_capacity(self.passes.len());

        for res in &self.passes {
            // A pass that owns an FBO (any intermediate, or a final pass with an
            // explicit scale — #22) sees its FBO size as OutputSize; a final pass
            // with no FBO sees the viewport. After `rebuild_chain`, an owning pass
            // always has its FBO.
            let output_size = match &res.fbo {
                Some(fbo) => fbo.size,
                None => viewport,
            };

            // No builtin block declared -> nothing to pack (the fallback vec4 UBO
            // stays zero), but still advance the running sizes. A block mixing
            // builtins + `#pragma parameter`s packs both here: builtins first
            // (offset-by-name), then the current param values overlaid at their
            // own offsets — one unified path (#29).
            if let Some(block) = &res.builtin_block {
                let values = BuiltinValues {
                    mvp: uniforms::ortho_mvp(),
                    source_size: uniforms::size_vec(input_size.0, input_size.1),
                    original_size: uniforms::size_vec(original.0, original.1),
                    output_size: uniforms::size_vec(output_size.0, output_size.1),
                    final_viewport_size: uniforms::size_vec(viewport.0, viewport.1),
                    frame_count: uniforms::apply_frame_count_mod(
                        self.frame_count,
                        res.frame_count_mod,
                    ),
                    frame_direction: self.frame_direction,
                    rotation: 0,
                    pass_output_sizes: pass_output_sizes.clone(),
                };
                let mut bytes = uniforms::pack_builtins(block, &values);
                uniforms::pack_params(&mut bytes, block, &self.params);
                self.queue.write_buffer(&res.ubo, 0, &bytes);
            }

            // The dedicated parameter block (binding 3): pack the current global
            // values at their reflected offsets and re-upload (#29). Re-done every
            // frame so a live `set_parameter` reaches the shader without a
            // recompile. A zero-filled buffer when the pass has no param block.
            if let Some(block) = &res.param_block {
                let mut bytes = vec![0u8; block.size as usize];
                uniforms::pack_params(&mut bytes, block, &self.params);
                self.queue.write_buffer(&res.param_buffer, 0, &bytes);
            }

            pass_output_sizes.push(uniforms::size_vec(output_size.0, output_size.1));
            input_size = output_size;
        }
    }

    /// Whether the chain is ready to draw (a source, a non-empty chain, and
    /// every pass's bind group built).
    fn ready(&self) -> bool {
        self.source.is_some()
            && !self.passes.is_empty()
            && self.passes.iter().all(|p| p.bind_group.is_some())
    }

    /// Render one frame: run every pass in order into its target (intermediate →
    /// owned FBO, final → offscreen). Requires a source + a chain.
    pub fn render(&mut self) -> Result<(), RendererError> {
        // Upgrade the source's mip chain if pass 0 reads it with `mipmap_input`
        // (#23/F) — before `rebuild_chain` so pass 0's bind group points at the
        // (possibly reallocated) source view.
        self.ensure_source_mips();
        self.rebuild_chain();
        if !self.ready() {
            return Err(RendererError::NotReady);
        }
        self.write_uniforms();

        // Build the mip-generation resources up front (lazily, once) for every
        // FBO format that needs mips this frame, so the per-pass loop below can
        // borrow `self.mip_gen` immutably alongside `self.passes`. The source's
        // own format is included when pass 0 needs source mips (#23/F).
        let mut mip_formats: Vec<wgpu::TextureFormat> = self
            .passes
            .iter()
            .filter(|p| p.produces_mips)
            .map(|p| p.target_format)
            .collect();
        if self.pass0_wants_source_mips() {
            mip_formats.push(Self::SOURCE_FORMAT);
        }
        if !mip_formats.is_empty() {
            self.ensure_mip_gen(&mip_formats);
        }

        // A final pass with an explicit scale renders into its own FBO and is then
        // stretched into the offscreen target (#22): build the blit resources up
        // front so the per-pass loop can borrow them immutably alongside `passes`.
        let final_owns_fbo = self.passes.last().is_some_and(|p| p.final_owns_fbo);
        if final_owns_fbo {
            self.ensure_blit();
        }

        let offscreen_view = self
            .offscreen
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame encoder"),
            });

        // Source mips (#23/F): if pass 0 reads the source with `mipmap_input`, the
        // source carries a mip chain; (re)generate it before pass 0 samples it
        // (§10 mip timing — analogous to a producing pass's FBO).
        if self.pass0_wants_source_mips() {
            if let Some(source) = &self.source {
                let mip_gen = self
                    .mip_gen
                    .as_ref()
                    .expect("mip_gen built when pass 0 wants source mips");
                generate_mips(&self.device, &mut encoder, mip_gen, source);
            }
        }

        for res in &self.passes {
            // Intermediate passes draw into their FBO's base mip level; the final
            // pass (no FBO) draws into the shared offscreen target. (Coarser mips,
            // if any, are filled by `generate_mips` after the draw.)
            let target = match &res.fbo {
                Some(fbo) => &fbo.base_view,
                None => &offscreen_view,
            };
            let bind_group = res
                .bind_group
                .as_ref()
                .expect("bind group built by rebuild_chain (checked in ready)");
            {
                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("chain pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                rp.set_pipeline(&res.pipeline);
                rp.set_bind_group(0, bind_group, &[]);
                rp.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                rp.draw(0..QUAD.len() as u32, 0..1);
            }
            // §10 mip timing: regenerate this FBO's mip chain immediately after
            // it is drawn, before the next (consuming) pass samples it.
            if res.produces_mips {
                if let Some(fbo) = &res.fbo {
                    let mip_gen = self
                        .mip_gen
                        .as_ref()
                        .expect("mip_gen built when any pass produces mips");
                    generate_mips(&self.device, &mut encoder, mip_gen, fbo);
                }
            }
            // #22: a final pass with an explicit scale drew into its own scaled
            // FBO; stretch it into the viewport-sized offscreen target with a
            // linear sampler so the read-back is the resampled result.
            if res.final_owns_fbo {
                if let Some(fbo) = &res.fbo {
                    let blit = self
                        .blit
                        .as_ref()
                        .expect("blit built when a final pass owns an FBO");
                    blit_to_offscreen(&self.device, &mut encoder, blit, fbo, &offscreen_view);
                }
            }
        }
        self.queue.submit(Some(encoder.finish()));
        // Advance the animation clock for the next frame (Phase 2 pumps this).
        self.frame_count = self.frame_count.wrapping_add(1);
        Ok(())
    }

    /// Lazily create the shared mip-generation resources and ensure a pipeline
    /// exists for each requested target format (#23). A render pipeline is
    /// format-specific, so we cache one per format.
    fn ensure_mip_gen(&mut self, formats: &[wgpu::TextureFormat]) {
        if self.mip_gen.is_none() {
            let module = self
                .device
                .create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("mipgen shader"),
                    source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(MIP_WGSL)),
                });
            let layout = self
                .device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("mipgen bind group layout"),
                    entries: &[
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        },
                    ],
                });
            let pipeline_layout =
                self.device
                    .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some("mipgen pipeline layout"),
                        bind_group_layouts: &[Some(&layout)],
                        immediate_size: 0,
                    });
            let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("mipgen sampler"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });
            self.mip_gen = Some(MipGen {
                module,
                layout,
                pipeline_layout,
                sampler,
                pipelines: std::collections::HashMap::new(),
            });
        }

        let mip_gen = self.mip_gen.as_mut().expect("just created");
        for &format in formats {
            mip_gen.pipelines.entry(format).or_insert_with(|| {
                self.device
                    .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some("mipgen pipeline"),
                        layout: Some(&mip_gen.pipeline_layout),
                        vertex: wgpu::VertexState {
                            module: &mip_gen.module,
                            entry_point: Some("vs"),
                            compilation_options: Default::default(),
                            buffers: &[],
                        },
                        fragment: Some(wgpu::FragmentState {
                            module: &mip_gen.module,
                            entry_point: Some("fs"),
                            compilation_options: Default::default(),
                            targets: &[Some(wgpu::ColorTargetState {
                                format,
                                blend: None,
                                write_mask: wgpu::ColorWrites::ALL,
                            })],
                        }),
                        primitive: wgpu::PrimitiveState::default(),
                        depth_stencil: None,
                        multisample: wgpu::MultisampleState::default(),
                        multiview_mask: None,
                        cache: None,
                    })
            });
        }
    }

    /// Lazily create the final-pass stretch-blit resources (#22): a passthrough
    /// pipeline (reusing [`MIP_WGSL`]) targeting [`OFFSCREEN_FORMAT`] plus a linear
    /// sampler, so a final pass's scaled FBO can be stretched into the offscreen
    /// target. A no-op after the first call.
    fn ensure_blit(&mut self) {
        if self.blit.is_some() {
            return;
        }
        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("blit shader"),
                source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(MIP_WGSL)),
            });
        let layout = self
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("blit bind group layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let pipeline_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("blit pipeline layout"),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blit sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("blit pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: OFFSCREEN_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });
        self.blit = Some(Blit {
            layout,
            sampler,
            pipeline,
        });
    }

    /// Frames rendered so far (the next `render` writes this as `FrameCount`).
    pub fn frame_count(&self) -> u32 {
        self.frame_count
    }

    /// The current `FrameDirection` (#28): `+1` forward, `-1` rewinding.
    pub fn frame_direction(&self) -> i32 {
        self.frame_direction
    }

    /// Set `FrameDirection` (#28/#31): `+1` forward, `-1` rewinding. Any nonzero
    /// value is taken as its sign; `0` is treated as forward.
    pub fn set_frame_direction(&mut self, direction: i32) {
        self.frame_direction = if direction < 0 { -1 } else { 1 };
    }

    /// Read the offscreen target (the final pass's output) back into a CPU
    /// [`Frame`] (RGBA8).
    pub fn read_back(&self) -> Result<Frame, RendererError> {
        let bytes_per_pixel = 4u32;
        let unpadded = self.width * bytes_per_pixel;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded = unpadded.div_ceil(align) * align;

        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (padded * self.height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("readback encoder"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.offscreen,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));

        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| RendererError::Readback(format!("{e:?}")))?;
        rx.recv()
            .map_err(|e| RendererError::Readback(e.to_string()))?
            .map_err(|e| RendererError::Readback(format!("{e:?}")))?;

        let data = slice.get_mapped_range();
        let mut rgba = Vec::with_capacity((unpadded * self.height) as usize);
        for row in 0..self.height {
            let start = (row * padded) as usize;
            rgba.extend_from_slice(&data[start..start + unpadded as usize]);
        }
        drop(data);
        buffer.unmap();

        Ok(Frame::new(self.width, self.height, rgba))
    }
}

impl Fbo {
    /// Allocate an intermediate render target of `size` and `format` with
    /// `mip_level_count` mip levels (#23). It is both a render attachment (this
    /// pass draws into it; mip-gen draws into each level) and texture-bound (the
    /// next pass samples it). The default `view` spans all mip levels so a
    /// `mipmap_input` consumer can sample coarse mips.
    fn allocate(
        device: &wgpu::Device,
        size: (u32, u32),
        format: wgpu::TextureFormat,
        mip_level_count: u32,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pass FBO"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let base_view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("pass FBO base level"),
            base_mip_level: 0,
            mip_level_count: Some(1),
            ..Default::default()
        });
        Self {
            texture,
            view,
            base_view,
            size,
            format,
            mip_level_count,
        }
    }
}

/// The set-0 binding number of a reflected uniform block, or `None` for a
/// push-constant block / absent block (#26). Used to attach the builtin/param UBO
/// buffers at their reflected bindings instead of the legacy fixed 0/3.
fn block_binding(block: Option<&UniformBlock>) -> Option<u32> {
    match block?.binding {
        BlockBinding::Uniform { set: 0, binding } => Some(binding),
        _ => None,
    }
}

/// Full mip-chain length for a 2D texture of `size`: `floor(log2(max(w,h))) + 1`.
fn mip_level_count_for(size: (u32, u32)) -> u32 {
    let max_dim = size.0.max(size.1).max(1);
    32 - max_dim.leading_zeros()
}

/// Regenerate `fbo`'s mip chain via a linear-blit downsample (#23): for each mip
/// level `k = 1 .. n`, sample level `k-1` with a linear sampler into level `k`
/// (a 2×2 average). Recorded into `encoder`, between the producing pass's draw
/// and the next pass's sample (§10 mip timing). No-op for a single-level FBO.
fn generate_mips(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    mip_gen: &MipGen,
    fbo: &Fbo,
) {
    if fbo.mip_level_count <= 1 {
        return;
    }
    let pipeline = mip_gen
        .pipelines
        .get(&fbo.format)
        .expect("mipgen pipeline for this format ensured before render");

    // One single-level view per mip level: used as the sampled source (level
    // k-1) and as the render target (level k).
    let level_views: Vec<wgpu::TextureView> = (0..fbo.mip_level_count)
        .map(|level| {
            fbo.texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("mip level view"),
                base_mip_level: level,
                mip_level_count: Some(1),
                ..Default::default()
            })
        })
        .collect();

    for level in 1..fbo.mip_level_count {
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mipgen bind group"),
            layout: &mip_gen.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &level_views[(level - 1) as usize],
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&mip_gen.sampler),
                },
            ],
        });
        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("mipgen pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &level_views[level as usize],
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        rp.set_pipeline(pipeline);
        rp.set_bind_group(0, &bind_group, &[]);
        rp.draw(0..3, 0..1);
    }
}

/// Stretch a final pass's scaled FBO into the viewport-sized offscreen target
/// (#22): a fullscreen-quad passthrough samples `fbo` (its full `view`, mip 0)
/// with a **linear** sampler and writes the offscreen target, so a final pass
/// declaring an explicit `scaleN` is resampled to the viewport before read-back.
fn blit_to_offscreen(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    blit: &Blit,
    fbo: &Fbo,
    offscreen_view: &wgpu::TextureView,
) {
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("blit bind group"),
        layout: &blit.layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&fbo.view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&blit.sampler),
            },
        ],
    });
    let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("final stretch blit"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: offscreen_view,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    rp.set_pipeline(&blit.pipeline);
    rp.set_bind_group(0, &bind_group, &[]);
    rp.draw(0..3, 0..1);
}

fn create_offscreen(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OFFSCREEN_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}
