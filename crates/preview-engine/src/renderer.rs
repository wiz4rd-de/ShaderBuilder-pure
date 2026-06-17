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

use crate::pass::{AxisScale, Pass, ScaleConfig};
use crate::uniforms::{self, BuiltinUniforms};
use slang_compile::CompiledShader;
use source::Frame;

/// The offscreen color format. Linear RGBA8 so a passthrough shader reproduces
/// the uploaded image byte-for-byte (no sRGB conversion in the slice). Phase 2
/// per-pass format selection (sRGB/float, #23) is reserved on [`Pass`].
pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

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
    /// Explicit scale config, or `None` to take the §2 position default.
    scale: Option<ScaleConfig>,
    /// `true` for an **intermediate** pass (owns an FBO, sized by its scale);
    /// `false` for the **final** pass, which renders into the shared offscreen
    /// target and has no FBO of its own.
    intermediate: bool,
    /// For an intermediate pass: its owned FBO, allocated on the first render and
    /// reallocated when its size changes. `None` for the final pass, and for an
    /// intermediate pass before its first allocation.
    fbo: Option<Fbo>,
    /// The bind group for this pass, rebuilt when its input texture changes.
    bind_group: Option<wgpu::BindGroup>,
}

/// An owned intermediate render target. Only the `view` is held — a wgpu
/// `TextureView` keeps its backing texture alive — plus the size it was
/// allocated at so we can detect when a viewport/source change needs a realloc.
struct Fbo {
    view: wgpu::TextureView,
    size: (u32, u32),
}

/// A headless N-pass renderer.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    vertex_buffer: wgpu::Buffer,
    sampler: wgpu::Sampler,
    bind_group_layout: wgpu::BindGroupLayout,

    width: u32,
    height: u32,
    offscreen: wgpu::Texture,

    /// Size of the uploaded source image, for the `*Size` uniforms.
    source_size: Option<(u32, u32)>,
    /// Frames rendered so far; written into `FrameCount` and bumped per frame.
    frame_count: u32,

    source_view: Option<wgpu::TextureView>,
    /// The ordered pass chain. Empty until [`Renderer::set_shader`]/`set_chain`.
    passes: Vec<PassResources>,
}

/// Size of the builtin UBO (std140; see [`BuiltinUniforms`]).
const UBO_SIZE: u64 = std::mem::size_of::<BuiltinUniforms>() as u64;

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

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("preview-engine device"),
            required_features: wgpu::Features::empty(),
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

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("source sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

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
            sampler,
            bind_group_layout,
            width,
            height,
            offscreen,
            source_size: None,
            frame_count: 0,
            source_view: None,
            passes: Vec::new(),
        })
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
            resources.push(self.build_pass(pass, is_final));
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
    fn build_pass(&self, pass: &Pass, is_final: bool) -> PassResources {
        let shader = &pass.shader;
        let params = uniforms::pack_parameters(&shader.reflection.parameters);
        let param_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("parameters"),
            size: params.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&param_buffer, 0, &params);

        let ubo = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniforms"),
            size: UBO_SIZE,
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

        // Every target uses OFFSCREEN_FORMAT for now; per-pass sRGB/float
        // formats are #23 (the reserved fields on `Pass`).
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

        PassResources {
            pipeline,
            param_buffer,
            ubo,
            scale: pass.scale,
            // The final pass renders into the offscreen target (no own FBO).
            intermediate: !is_final,
            fbo: None,
            bind_group: None,
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
                if res.fbo.as_ref().map(|f| f.size) != Some(size) {
                    res.fbo = Some(Fbo::allocate(&self.device, size));
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
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
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

    /// Compute and upload each pass's builtin UBO for the current frame. A pass's
    /// `Source`/`Original`/`Output` sizes follow the §2 chaining: pass 0's input
    /// is the source image (== `Original`); pass i's input is FBO i-1; the output
    /// is the pass's own target size.
    fn write_uniforms(&self) {
        let original = self.source_size.unwrap_or((self.width, self.height));
        let viewport = (self.width, self.height);
        let mut input_size = original;
        for res in &self.passes {
            // After `rebuild_chain`, an intermediate pass always has its FBO.
            let output_size = match (res.intermediate, &res.fbo) {
                (true, Some(fbo)) => fbo.size,
                _ => viewport,
            };
            let builtins =
                BuiltinUniforms::new_full(input_size, original, output_size, self.frame_count);
            self.queue.write_buffer(&res.ubo, 0, builtins.as_bytes());
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

        let offscreen_view = self
            .offscreen
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame encoder"),
            });
        for res in &self.passes {
            // Intermediate passes draw into their FBO; the final pass (no FBO)
            // draws into the shared offscreen target.
            let target = match &res.fbo {
                Some(fbo) => &fbo.view,
                None => &offscreen_view,
            };
            let bind_group = res
                .bind_group
                .as_ref()
                .expect("bind group built by rebuild_chain (checked in ready)");
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
        self.queue.submit(Some(encoder.finish()));
        // Advance the animation clock for the next frame (Phase 2 pumps this).
        self.frame_count = self.frame_count.wrapping_add(1);
        Ok(())
    }

    /// Frames rendered so far (the next `render` writes this as `FrameCount`).
    pub fn frame_count(&self) -> u32 {
        self.frame_count
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
    /// Allocate an intermediate render target of `size`. It is both a render
    /// attachment (this pass draws into it) and texture-bound (the next pass
    /// samples it).
    fn allocate(device: &wgpu::Device, size: (u32, u32)) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pass FBO"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self { view, size }
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
