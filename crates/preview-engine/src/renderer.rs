//! Headless wgpu renderer: a single offscreen pass that draws a fullscreen quad
//! sampling the source image through a compiled slang shader (Architecture
//! §D/§F). No window — renders to an offscreen RGBA8 target and reads it back.
//!
//! SPIR-V from `slang-compile` is ingested via `wgpu::ShaderSource::SpirV` (no
//! WGSL hop). The bind group is the minimal one-pass set: a uniform buffer
//! (`MVP` here; the full builtin set is added in #19), the source `texture2D`,
//! and a `sampler`.
//!
//! Note on samplers: wgpu's binding model uses **separate** texture + sampler,
//! not GLSL's combined `sampler2D`. Phase 1 fixtures therefore use separate
//! samplers; converting real RetroArch combined-`sampler2D` shaders is a Phase 2
//! import concern.

use slang_compile::CompiledShader;
use source::Frame;

/// The offscreen color format. Linear RGBA8 so a passthrough shader reproduces
/// the uploaded image byte-for-byte (no sRGB conversion in the slice).
pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Fullscreen-quad vertex: clip-space position + source texture coordinate.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 4],
    uv: [f32; 2],
}

/// Two triangles covering the viewport. UVs are set so the image's top-left
/// lands at the framebuffer's top-left (wgpu NDC is y-up; texture row 0 is top).
const QUAD: [Vertex; 6] = [
    Vertex {
        pos: [-1.0, -1.0, 0.0, 1.0],
        uv: [0.0, 1.0],
    },
    Vertex {
        pos: [1.0, -1.0, 0.0, 1.0],
        uv: [1.0, 1.0],
    },
    Vertex {
        pos: [1.0, 1.0, 0.0, 1.0],
        uv: [1.0, 0.0],
    },
    Vertex {
        pos: [-1.0, -1.0, 0.0, 1.0],
        uv: [0.0, 1.0],
    },
    Vertex {
        pos: [1.0, 1.0, 0.0, 1.0],
        uv: [1.0, 0.0],
    },
    Vertex {
        pos: [-1.0, 1.0, 0.0, 1.0],
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
    /// Reading back the offscreen target failed.
    Readback(String),
}

impl std::fmt::Display for RendererError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RendererError::NoAdapter => write!(f, "no suitable wgpu adapter found"),
            RendererError::Device(e) => write!(f, "failed to request wgpu device: {e}"),
            RendererError::NotReady => write!(f, "renderer needs both a source image and a shader"),
            RendererError::Readback(e) => write!(f, "offscreen readback failed: {e}"),
        }
    }
}

impl std::error::Error for RendererError {}

/// A headless single-pass renderer.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    vertex_buffer: wgpu::Buffer,
    /// Uniform buffer (MVP for now; grown to the full builtin set in #19).
    ubo: wgpu::Buffer,
    sampler: wgpu::Sampler,
    bind_group_layout: wgpu::BindGroupLayout,

    width: u32,
    height: u32,
    offscreen: wgpu::Texture,

    source_view: Option<wgpu::TextureView>,
    pipeline: Option<wgpu::RenderPipeline>,
    bind_group: Option<wgpu::BindGroup>,
}

/// Bytes the UBO holds in Phase 1: a single `mat4 MVP`. (#19 grows this.)
const UBO_SIZE: u64 = 64;

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

        let ubo = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniforms"),
            size: UBO_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

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
            ],
        });

        let offscreen = create_offscreen(&device, width, height);

        Ok(Self {
            device,
            queue,
            vertex_buffer,
            ubo,
            sampler,
            bind_group_layout,
            width,
            height,
            offscreen,
            source_view: None,
            pipeline: None,
            bind_group: None,
        })
    }

    /// The current offscreen target size.
    pub fn viewport(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Resize the offscreen target.
    pub fn set_viewport(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if (width, height) != (self.width, self.height) {
            self.width = width;
            self.height = height;
            self.offscreen = create_offscreen(&self.device, width, height);
        }
    }

    /// Upload a source image into a sampled texture and (re)build the bind group.
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
        self.rebuild_bind_group();
    }

    /// Build the render pipeline from the compiled SPIR-V modules.
    pub fn set_shader(&mut self, shader: &CompiledShader) {
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

        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("single pass"),
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
        self.pipeline = Some(pipeline);
    }

    /// Write the current uniforms (identity MVP in Phase 1; #19 fills the rest).
    fn write_uniforms(&self) {
        let identity: [f32; 16] = [
            1.0, 0.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ];
        self.queue
            .write_buffer(&self.ubo, 0, bytemuck::cast_slice(&identity));
    }

    fn rebuild_bind_group(&mut self) {
        let Some(view) = &self.source_view else {
            self.bind_group = None;
            return;
        };
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pass bind group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.ubo.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.bind_group = Some(bind_group);
    }

    /// Render one frame into the offscreen target. Requires a source + shader.
    pub fn render(&mut self) -> Result<(), RendererError> {
        let (Some(pipeline), Some(bind_group)) = (&self.pipeline, &self.bind_group) else {
            return Err(RendererError::NotReady);
        };
        self.write_uniforms();

        let view = self
            .offscreen
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("single pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind_group, &[]);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.draw(0..QUAD.len() as u32, 0..1);
        }
        self.queue.submit(Some(encoder.finish()));
        Ok(())
    }

    /// Read the offscreen target back into a CPU [`Frame`] (RGBA8).
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
