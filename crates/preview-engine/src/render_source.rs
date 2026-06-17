//! Live preview producer: a [`FrameSource`] backed by the headless wgpu
//! [`Renderer`]. This is what replaces Phase 0's [`crate::GradientSource`] —
//! the app's `tauri::ipc::Channel` transport drives it through the very same
//! [`FrameSource`] seam (Architecture §F).
//!
//! Each [`FrameSource::render_into`] call first drains any pending
//! [`RenderCommand`]s (sent from the app's Tauri commands over an mpsc channel),
//! then renders one frame into the offscreen target and reads it back into a
//! binary preview frame. The offscreen target is the **preview-pane size**, so
//! sampling a larger source image through the shader downsamples it on the GPU
//! to the pane — bounding the per-frame transfer cost regardless of how large
//! the source or the simulated viewport gets (Architecture §F). A configurable
//! *higher-than-pane* simulated viewport with an explicit box-filter downsample
//! is a Phase 2 refinement.
//!
//! Until both a source image and a shader have been loaded, `render_into`
//! emits a solid "waiting" frame at the pane size so the transport and canvas
//! stay exercised end-to-end (and so a missing/!ready state never starves the
//! stream).

use std::sync::mpsc::Receiver;

use slang_compile::CompiledShader;
use source::Frame;

use crate::frame::{FrameHeader, FRAME_HEADER_LEN};
use crate::pass::Pass;
use crate::renderer::{Renderer, RendererError};
use crate::FrameSource;

/// The built-in default preview shader: a passthrough that samples the source
/// 1:1, using **separate** texture + sampler as wgpu's binding model requires
/// (see [`crate::renderer`]). Compiled by the app when a preview is requested
/// without a `.slang` file selected yet, so the pane shows the real source
/// image rather than nothing.
pub const DEFAULT_SHADER: &str = "\
#version 450
layout(std140, set = 0, binding = 0) uniform UBO { mat4 MVP; } global;
#pragma stage vertex
layout(location = 0) in vec4 Position;
layout(location = 1) in vec2 TexCoord;
layout(location = 0) out vec2 vTexCoord;
void main() { gl_Position = global.MVP * Position; vTexCoord = TexCoord; }
#pragma stage fragment
layout(location = 0) in vec2 vTexCoord;
layout(location = 0) out vec4 FragColor;
layout(set = 0, binding = 1) uniform texture2D Source;
layout(set = 0, binding = 2) uniform sampler Smp;
void main() { FragColor = texture(sampler2D(Source, Smp), vTexCoord); }
";

/// The solid color emitted before a source + shader are loaded — a neutral dark
/// gray, distinct from a black clear and from the Phase 0 gradient.
const WAITING_RGBA: [u8; 4] = [32, 32, 32, 255];

/// A command driving the live preview renderer. Produced by the app's Tauri
/// commands and applied to the [`RenderSource`] on the render thread between
/// frames (so heavy IO/compile happens off this loop — the app does it before
/// sending).
pub enum RenderCommand {
    /// Replace the source image the shader samples.
    SetSource(Frame),
    /// Replace the active shader with a single-pass chain (back-compat with the
    /// Phase-1 single-shader path).
    SetShader(CompiledShader),
    /// Replace the active render chain with an ordered N-pass pipeline (#22). A
    /// `.slangp` preset is loaded by the app into a `Vec<Pass>` and sent here.
    SetChain(Vec<Pass>),
    /// Resize the offscreen target — i.e. the preview-pane size that frames are
    /// downsampled to.
    SetViewport(u32, u32),
    /// Set (or clear) the **simulated viewport** (#30): the output resolution +
    /// integer-scale the final pass renders at, distinct from the pane. `None`
    /// makes the viewport track the pane (the default). When `Some`, `viewport`-
    /// scaled FBOs, the final pass's `OutputSize`, and `FinalViewportSize` reflect
    /// the §9 content rect, which is composited (with letterbox bars) into the pane.
    SetSimulatedViewport(Option<crate::ViewportConfig>),
    /// Update a `#pragma parameter`'s current value live (#29). Applied to the
    /// chain's global-by-name parameter store; the next frame re-packs it — no
    /// recompile or pipeline rebuild. An unknown name is a no-op.
    SetParameter { name: String, value: f32 },
    /// Apply a preset's `parameter_overrides` (#29): for each `name -> value`, set
    /// the current value (clamped). Sent after a `SetChain` so the override lands
    /// on the freshly-collected defaults.
    ApplyParameterOverrides(std::collections::BTreeMap<String, f32>),
    /// Replace the registered LUTs (#27): the preset's `textures` family, decoded
    /// by the app and bound by name as `<NAME>`. An empty list clears stale LUTs.
    SetLuts(Vec<crate::LutSpec>),
}

/// A [`FrameSource`] that renders a source image through a compiled slang shader
/// on wgpu and reads the result back as a pane-sized binary frame.
pub struct RenderSource {
    renderer: Renderer,
    commands: Receiver<RenderCommand>,
    have_source: bool,
    have_shader: bool,
}

impl RenderSource {
    /// Initialize the wgpu renderer at the given pane size. The heavy device
    /// init runs here, so construct this on the render thread (keeping the
    /// `start_preview_stream` command non-blocking). `commands` is the receiving
    /// end of the channel the app's load/viewport commands send on.
    pub fn new(
        width: u32,
        height: u32,
        commands: Receiver<RenderCommand>,
    ) -> Result<Self, RendererError> {
        Ok(Self {
            renderer: Renderer::new(width, height)?,
            commands,
            have_source: false,
            have_shader: false,
        })
    }

    /// Drain and apply every queued command. Loads are applied in arrival order;
    /// the actual render happens once afterwards, so source/shader/viewport
    /// updates in the same batch all take effect on the next frame together.
    fn apply_commands(&mut self) {
        while let Ok(cmd) = self.commands.try_recv() {
            match cmd {
                RenderCommand::SetSource(frame) => {
                    self.renderer.set_source(&frame);
                    self.have_source = true;
                }
                RenderCommand::SetShader(shader) => {
                    self.renderer.set_shader(&shader);
                    self.have_shader = true;
                }
                RenderCommand::SetChain(passes) => {
                    // An empty chain is ignored (keeps the previous one running).
                    if self.renderer.set_chain(&passes).is_ok() {
                        self.have_shader = true;
                    }
                }
                RenderCommand::SetViewport(width, height) => {
                    self.renderer.set_viewport(width, height);
                }
                RenderCommand::SetSimulatedViewport(config) => {
                    // #30: takes effect next frame — `viewport`-scaled FBOs and the
                    // final-pass composite recompute from the new content rect.
                    self.renderer.set_simulated_viewport(config);
                }
                RenderCommand::SetParameter { name, value } => {
                    // Live param update: no recompile, takes effect next frame.
                    self.renderer.set_parameter(&name, value);
                }
                RenderCommand::ApplyParameterOverrides(overrides) => {
                    let mut store = self.renderer.collected_params().clone();
                    store.apply_overrides(&overrides);
                    self.renderer.set_params(store);
                }
                RenderCommand::SetLuts(luts) => {
                    self.renderer.set_luts(luts);
                }
            }
        }
    }

    /// Whether a real render can run (both a source image and a shader are set).
    fn ready(&self) -> bool {
        self.have_source && self.have_shader
    }
}

impl FrameSource for RenderSource {
    fn dimensions(&self) -> (u32, u32) {
        self.renderer.viewport()
    }

    fn render_into(&mut self, index: u64, buf: &mut Vec<u8>) {
        self.apply_commands();
        let (width, height) = self.renderer.viewport();
        buf.clear();

        // Render one frame and read it back; on any failure fall through to the
        // waiting frame so the stream keeps a valid, pane-sized output.
        if self.ready() && self.renderer.render().is_ok() {
            if let Ok(frame) = self.renderer.read_back() {
                let header = FrameHeader::rgba8(frame.width, frame.height, index);
                buf.reserve(FRAME_HEADER_LEN + header.payload_len());
                header.write_to(buf);
                buf.extend_from_slice(&frame.rgba);
                return;
            }
        }

        write_solid_frame(buf, width, height, index, WAITING_RGBA);
    }
}

/// Write a header + a solid-color RGBA8 payload of `width * height` pixels.
fn write_solid_frame(buf: &mut Vec<u8>, width: u32, height: u32, index: u64, rgba: [u8; 4]) {
    let header = FrameHeader::rgba8(width, height, index);
    buf.reserve(FRAME_HEADER_LEN + header.payload_len());
    header.write_to(buf);
    for _ in 0..(width as usize * height as usize) {
        buf.extend_from_slice(&rgba);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{FRAME_MAGIC, PIXEL_FORMAT_RGBA8};
    use slang_compile::compile_slang;
    use source::test_pattern;
    use std::sync::mpsc;

    /// Read `(width, height)` out of a binary frame header.
    fn frame_dims(buf: &[u8]) -> (u32, u32) {
        assert_eq!(&buf[0..4], &FRAME_MAGIC, "frame magic");
        assert_eq!(buf[6], PIXEL_FORMAT_RGBA8, "pixel format");
        let w = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
        let h = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
        (w, h)
    }

    fn pixels(buf: &[u8]) -> &[u8] {
        &buf[FRAME_HEADER_LEN..]
    }

    #[test]
    fn default_shader_compiles() {
        // The built-in passthrough must survive the real toolchain.
        let shader = compile_slang(DEFAULT_SHADER, None).expect("default shader compiles");
        assert!(shader.reflection.parameters.is_empty());
    }

    #[test]
    fn downsamples_large_source_to_pane_size() {
        // Pane 32x24, source 128x128 -> the streamed frame is pane-sized, not
        // source-sized: the GPU downsamples while sampling.
        let (tx, rx) = mpsc::channel();
        let mut src = RenderSource::new(32, 24, rx).expect("wgpu device");

        let shader = compile_slang(DEFAULT_SHADER, None).expect("compile");
        tx.send(RenderCommand::SetShader(shader)).unwrap();
        tx.send(RenderCommand::SetSource(test_pattern(128, 128)))
            .unwrap();

        let mut buf = Vec::new();
        src.render_into(0, &mut buf);

        assert_eq!(frame_dims(&buf), (32, 24), "frame is pane-sized");
        assert_eq!(buf.len(), FRAME_HEADER_LEN + 32 * 24 * 4, "payload size");
        // The render is real: the high-contrast test pattern is not a solid fill.
        let px = pixels(&buf);
        assert!(
            px.chunks_exact(4).any(|p| p != &px[0..4]),
            "expected varied pixels from a real render, got a solid frame"
        );
        assert_ne!(
            &px[0..4],
            WAITING_RGBA.as_slice(),
            "should not be the waiting frame"
        );
    }

    #[test]
    fn emits_waiting_frame_before_ready() {
        // No source/shader loaded: a valid, pane-sized, solid waiting frame.
        let (_tx, rx) = mpsc::channel();
        let mut src = RenderSource::new(8, 6, rx).expect("wgpu device");

        let mut buf = Vec::new();
        src.render_into(0, &mut buf);

        assert_eq!(frame_dims(&buf), (8, 6));
        assert_eq!(buf.len(), FRAME_HEADER_LEN + 8 * 6 * 4);
        let px = pixels(&buf);
        assert!(
            px.chunks_exact(4).all(|p| p == WAITING_RGBA.as_slice()),
            "every pixel should be the waiting color until ready"
        );
    }

    /// A param-only shader (R = X) driven over the command channel: the
    /// `SetParameter` command must change the streamed frame within one render,
    /// exactly as the Tauri command will (#29).
    #[test]
    fn set_parameter_command_changes_streamed_frame() {
        let (tx, rx) = mpsc::channel();
        let mut src = RenderSource::new(8, 8, rx).expect("wgpu device");

        let shader = compile_slang(
            "#version 450
#pragma parameter X \"X\" 0.5 0.0 1.0 0.01
layout(std140, set = 0, binding = 0) uniform UBO { mat4 MVP; } global;
layout(std140, set = 0, binding = 3) uniform Params { float X; } params;
#pragma stage vertex
layout(location = 0) in vec4 Position;
layout(location = 1) in vec2 TexCoord;
void main() { gl_Position = global.MVP * Position; }
#pragma stage fragment
layout(location = 0) out vec4 FragColor;
layout(set = 0, binding = 1) uniform texture2D Source;
layout(set = 0, binding = 2) uniform sampler Smp;
void main() { FragColor = vec4(params.X, 0.0, 0.0, 1.0); }
",
            None,
        )
        .expect("compile param shader");
        tx.send(RenderCommand::SetShader(shader)).unwrap();
        tx.send(RenderCommand::SetSource(test_pattern(8, 8)))
            .unwrap();

        // Default X=0.5 -> R~128.
        let mut buf = Vec::new();
        src.render_into(0, &mut buf);
        assert!((pixels(&buf)[0] as i32 - 128).abs() <= 4, "default X");

        // Live set X=0.9 -> R~230 on the very next frame.
        tx.send(RenderCommand::SetParameter {
            name: "X".to_string(),
            value: 0.9,
        })
        .unwrap();
        src.render_into(1, &mut buf);
        assert!(
            (pixels(&buf)[0] as i32 - 230).abs() <= 6,
            "live SetParameter took effect, got {}",
            pixels(&buf)[0]
        );
    }

    #[test]
    fn set_viewport_command_resizes_emitted_frames() {
        let (tx, rx) = mpsc::channel();
        let mut src = RenderSource::new(16, 16, rx).expect("wgpu device");

        let shader = compile_slang(DEFAULT_SHADER, None).expect("compile");
        tx.send(RenderCommand::SetShader(shader)).unwrap();
        tx.send(RenderCommand::SetSource(test_pattern(64, 64)))
            .unwrap();
        tx.send(RenderCommand::SetViewport(40, 30)).unwrap();

        let mut buf = Vec::new();
        src.render_into(0, &mut buf);
        assert_eq!(frame_dims(&buf), (40, 30), "viewport command took effect");
    }
}
