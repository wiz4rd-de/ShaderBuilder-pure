//! Headless wgpu renderer: an ordered **N-pass** chain that draws a fullscreen
//! quad sampling the source image through each compiled slang pass in turn
//! (Architecture §D/§F; `docs/retroarch-slang-runtime.md` §2/§10). No window —
//! the final pass renders to an offscreen RGBA8 target that is read back.
//!
//! ## Chain model (#22)
//! Pass 0's `Source` is the input [`Frame`] (`Original`); pass `i`'s `Source` is
//! pass `i-1`'s output texture. Intermediate passes render into **owned FBOs**
//! sized by their scale type (§2); the **final pass renders into the
//! viewport/pane** (the offscreen target, preserving the existing read-back /
//! downsample path). FBO sizes are recomputed whenever the viewport or source
//! size changes.
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
//! Note on samplers: wgpu's binding model uses **separate** texture + sampler,
//! not GLSL's combined `sampler2D` (the conversion happens in `slang-compile`).

use crate::pass::{AxisScale, Pass, ScaleConfig, WrapMode};
use crate::uniforms::{self, BuiltinValues};
use slang_compile::{CompiledShader, UniformBlock};
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

/// GPU resources for one pass in the chain.
struct PassResources {
    pipeline: wgpu::RenderPipeline,
    /// Parameter UBO (`#pragma parameter` defaults).
    param_buffer: wgpu::Buffer,
    /// This pass's builtin UBO (per-pass because `*Size` differs per pass).
    ubo: wgpu::Buffer,
    /// The reflected builtin uniform block this pass declares (#28): member
    /// names → byte offsets, discovered from the SPIR-V so builtins can be
    /// declared in any order/subset. `None` if the pass declares no builtin
    /// block (e.g. a constant-color pass); then the UBO is a zero-filled vec4.
    builtin_block: Option<UniformBlock>,
    /// `frame_count_modN` (#28): the `FrameCount` this pass sees is pre-wrapped
    /// by this modulus (`0` = no wrap).
    frame_count_mod: u32,
    /// This pass's **input** sampler (binding 2): filter/wrap/mip per the pass's
    /// `filter_linear`/`wrap_mode`/`mipmap_input` (#23). Built once at
    /// [`Renderer::build_pass`].
    sampler: wgpu::Sampler,
    /// The render-target format this pass writes (its FBO format for an
    /// intermediate pass; [`OFFSCREEN_FORMAT`] for the final pass). The pipeline
    /// was built for this format, so the FBO must match it.
    target_format: wgpu::TextureFormat,
    /// Explicit scale config, or `None` to take the §2 position default.
    scale: Option<ScaleConfig>,
    /// `true` for an **intermediate** pass (owns an FBO, sized by its scale);
    /// `false` for the **final** pass, which renders into the shared offscreen
    /// target and has no FBO of its own.
    intermediate: bool,
    /// `true` if a **downstream** consumer reads this pass's output with
    /// `mipmap_input` (#23): its FBO must carry a full mip chain that we
    /// regenerate each frame right after this pass draws (§10 mip timing).
    produces_mips: bool,
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
    bind_group_layout: wgpu::BindGroupLayout,
    /// Whether the device supports `AddressMode::ClampToBorder` (#23). When
    /// `false`, a pass asking for `WrapMode::ClampToBorder` falls back to
    /// `ClampToEdge` (so lavapipe/CI without the feature still works).
    clamp_to_border_supported: bool,
    /// Lazily-built resources for mipmap generation (pipeline + sampler), shared
    /// across passes and formats keyed by target format.
    mip_gen: Option<MipGen>,

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

    source_view: Option<wgpu::TextureView>,
    /// The ordered pass chain. Empty until [`Renderer::set_shader`]/`set_chain`.
    passes: Vec<PassResources>,
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

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pass bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // Parameter UBO. Present even for shaders that declare no
                // parameters: a layout may carry bindings the shader doesn't use.
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let offscreen = create_offscreen(&device, width, height);

        Ok(Self {
            device,
            queue,
            vertex_buffer,
            bind_group_layout,
            clamp_to_border_supported,
            mip_gen: None,
            width,
            height,
            offscreen,
            source_size: None,
            frame_count: 0,
            frame_direction: 1,
            source_view: None,
            passes: Vec::new(),
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

    /// Upload a source image into a sampled texture and (re)build pass 0's bind
    /// group (its input is the source).
    pub fn set_source(&mut self, frame: &Frame) {
        let size = wgpu::Extent3d {
            width: frame.width,
            height: frame.height,
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("source image"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &frame.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(frame.width * 4),
                rows_per_image: Some(frame.height),
            },
            size,
        );
        self.source_view = Some(texture.create_view(&wgpu::TextureViewDescriptor::default()));
        self.source_size = Some((frame.width, frame.height));
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
        let last = passes.len() - 1;
        let mut resources = Vec::with_capacity(passes.len());
        for (i, pass) in passes.iter().enumerate() {
            let is_final = i == last;
            // Pass `i`'s output is sampled with mips iff the *next* pass reads it
            // with `mipmap_input` (its input is this pass's FBO). The final pass
            // never produces mips (nothing reads it as Source).
            let produces_mips = !is_final && passes[i + 1].mipmap_input;
            resources.push(self.build_pass(pass, is_final, produces_mips));
        }
        self.passes = resources;
        // FBO sizes + bind groups are (re)built lazily at render time, once the
        // source size is known and the full chain exists.
        Ok(())
    }

    /// Build the GPU resources for one pass: pipeline, parameter UBO (from the
    /// shader's reflected `#pragma parameter` defaults), the per-pass builtin
    /// UBO, and — for an intermediate pass — a placeholder FBO slot (sized on the
    /// first render).
    fn build_pass(&self, pass: &Pass, is_final: bool, produces_mips: bool) -> PassResources {
        let shader = &pass.shader;

        // The format this pass writes: the final pass always writes the 8-bit
        // read-back target (OFFSCREEN_FORMAT); an intermediate pass writes its
        // selected per-pass format (§3). The pipeline's color target MUST match
        // the FBO it renders into, so build the pipeline for this format.
        let target_format = if is_final {
            OFFSCREEN_FORMAT
        } else {
            pass.fbo_format()
        };

        // This pass's input sampler (binding 2): filter + wrap + mip per its
        // `filter_linear`/`wrap_mode`/`mipmap_input` (#23). `mipmap_input` raises
        // `lod_max_clamp` and selects a linear mipmap filter so coarse mips are
        // sampled; otherwise lod is clamped to the base level.
        let sampler = self.build_sampler(pass);
        let params = uniforms::pack_parameters(&shader.reflection.parameters);
        let param_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("parameters"),
            size: params.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&param_buffer, 0, &params);

        // Reflect the SPIR-V to discover the builtin block's member offsets (#28)
        // so builtins can be declared in any order/subset. A reflection failure
        // is non-fatal: fall back to no builtin block (zero-filled UBO) rather
        // than refusing to build the pass.
        let builtin_block = slang_compile::reflect(shader)
            .ok()
            .and_then(|r| uniforms::builtin_block(&r).cloned());
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
                bind_group_layouts: &[Some(&self.bind_group_layout)],
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

        PassResources {
            pipeline,
            param_buffer,
            ubo,
            builtin_block,
            frame_count_mod: pass.frame_count_mod,
            sampler,
            target_format,
            scale: pass.scale,
            // The final pass renders into the offscreen target (no own FBO).
            intermediate: !is_final,
            produces_mips,
            fbo: None,
            bind_group: None,
        }
    }

    /// Build a pass's input sampler from its `filter_linear`/`wrap_mode`/
    /// `mipmap_input` (#23). Address modes use [`Renderer::address_mode`] (which
    /// applies the ClampToBorder→ClampToEdge fallback). When `mipmap_input` is
    /// set, the mipmap filter is `Linear` and `lod_max_clamp` is left at its
    /// default (`f32::MAX`) so coarse mips can be sampled; otherwise lod is
    /// clamped to the base level so only mip 0 is read.
    fn build_sampler(&self, pass: &Pass) -> wgpu::Sampler {
        let filter = if pass.filter_linear {
            wgpu::FilterMode::Linear
        } else {
            wgpu::FilterMode::Nearest
        };
        let address = self.address_mode(pass.wrap_mode);
        let (mipmap_filter, lod_max_clamp) = if pass.mipmap_input {
            // Linear between mip levels; allow the full chain.
            (wgpu::MipmapFilterMode::Linear, f32::MAX)
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

    /// (Re)allocate intermediate FBOs to the sizes the current source + viewport
    /// imply, and wire each pass's bind group to its input texture (pass 0 ←
    /// source; pass i ← pass i-1's FBO). Called at the top of [`render`].
    fn rebuild_chain(&mut self) {
        let Some(source_view) = &self.source_view else {
            return;
        };
        let source_size = self.source_size.unwrap_or((self.width, self.height));
        let viewport = (self.width, self.height);

        // Pass 1: resolve + (re)allocate every intermediate FBO, tracking the
        // running input size down the chain (§2: a `source` scale on pass n is
        // relative to FBO n-1, not to Original).
        let mut input_size = source_size;
        for res in &mut self.passes {
            if res.intermediate {
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
                // Final pass: output is the viewport.
                input_size = viewport;
            }
        }

        // Pass 2: build each pass's bind group, chaining input views. Each pass's
        // input is the previous FBO view (or the source for pass 0).
        let mut prev_view: Option<&wgpu::TextureView> = None;
        let count = self.passes.len();
        for i in 0..count {
            let input_view = match prev_view {
                None => source_view,
                Some(v) => v,
            };
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("pass bind group"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.passes[i].ubo.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(input_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.passes[i].sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: self.passes[i].param_buffer.as_entire_binding(),
                    },
                ],
            });
            self.passes[i].bind_group = Some(bind_group);
            // Next pass's input is this pass's FBO output (final pass has none,
            // and is always last, so prev_view is irrelevant after it).
            prev_view = self.passes[i].fbo.as_ref().map(|f| &f.view);
        }
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
            // After `rebuild_chain`, an intermediate pass always has its FBO.
            let output_size = match (res.intermediate, &res.fbo) {
                (true, Some(fbo)) => fbo.size,
                _ => viewport,
            };

            // No builtin block declared -> nothing to pack (the fallback vec4 UBO
            // stays zero), but still advance the running sizes.
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
                let bytes = uniforms::pack_builtins(block, &values);
                self.queue.write_buffer(&res.ubo, 0, &bytes);
            }

            pass_output_sizes.push(uniforms::size_vec(output_size.0, output_size.1));
            input_size = output_size;
        }
    }

    /// Whether the chain is ready to draw (a source, a non-empty chain, and
    /// every pass's bind group built).
    fn ready(&self) -> bool {
        self.source_view.is_some()
            && !self.passes.is_empty()
            && self.passes.iter().all(|p| p.bind_group.is_some())
    }

    /// Render one frame: run every pass in order into its target (intermediate →
    /// owned FBO, final → offscreen). Requires a source + a chain.
    pub fn render(&mut self) -> Result<(), RendererError> {
        self.rebuild_chain();
        if !self.ready() {
            return Err(RendererError::NotReady);
        }
        self.write_uniforms();

        // Build the mip-generation resources up front (lazily, once) for every
        // FBO format that needs mips this frame, so the per-pass loop below can
        // borrow `self.mip_gen` immutably alongside `self.passes`.
        let mip_formats: Vec<wgpu::TextureFormat> = self
            .passes
            .iter()
            .filter(|p| p.produces_mips)
            .map(|p| p.target_format)
            .collect();
        if !mip_formats.is_empty() {
            self.ensure_mip_gen(&mip_formats);
        }

        let offscreen_view = self
            .offscreen
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame encoder"),
            });
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
