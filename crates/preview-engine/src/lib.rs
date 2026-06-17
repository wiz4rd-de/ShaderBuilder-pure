//! `preview-engine` — the core: a faithful re-implementation of RetroArch's
//! slang runtime on wgpu. Owns the device/queue + source pump on a dedicated
//! render thread, builds the per-pass resource graph (scale types, FBO formats,
//! samplers, feedback double-buffers, history ring, LUTs), computes all builtin
//! semantics, and renders into the simulated viewport (Architecture §D).
//!
//! Phase 0: no GPU yet. What exists here is the **frame transport seam** — the
//! binary frame format ([`frame`]) and the [`FrameSource`] trait — plus a dummy
//! [`GradientSource`] so the offscreen-render → stream-binary-frames path
//! (Architecture §F) is proven end-to-end before any real rendering. Phase 1
//! swaps in a wgpu-backed `FrameSource` without changing the `app` transport.

pub mod frame;
pub mod renderer;

pub use frame::{FrameHeader, FRAME_HEADER_LEN, FRAME_MAGIC, FRAME_VERSION, PIXEL_FORMAT_RGBA8};
pub use renderer::{Renderer, RendererError, OFFSCREEN_FORMAT};

/// Crate identity marker. See `core_model::NAME`.
pub const NAME: &str = "preview-engine";

/// A producer of preview frames.
///
/// This is the **swap seam** for the preview pipeline. Phase 0 ships the dummy
/// [`GradientSource`]; Phase 1 replaces it with the offscreen wgpu renderer
/// implementing this same trait. The `app` crate's `tauri::ipc::Channel`
/// transport depends only on this trait, so swapping producers needs no IPC
/// changes.
pub trait FrameSource: Send {
    /// The `(width, height)` of frames this source produces.
    fn dimensions(&self) -> (u32, u32);

    /// Render frame `index` as a complete binary frame (24-byte header + RGBA8)
    /// into `buf`, which is cleared first. See [`frame`] for the layout.
    fn render_into(&mut self, index: u64, buf: &mut Vec<u8>);
}

/// **Placeholder** preview source: a CPU-generated animated gradient.
///
/// Exists purely to exercise the frame transport (Architecture §F) before any
/// GPU work. Replaced by the real wgpu renderer in Phase 1.
#[derive(Debug, Clone, Copy)]
pub struct GradientSource {
    width: u32,
    height: u32,
}

impl GradientSource {
    /// A gradient source of the given size (clamped to at least 1×1).
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width: width.max(1),
            height: height.max(1),
        }
    }
}

impl FrameSource for GradientSource {
    fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn render_into(&mut self, index: u64, buf: &mut Vec<u8>) {
        let header = FrameHeader::rgba8(self.width, self.height, index);
        buf.clear();
        buf.reserve(FRAME_HEADER_LEN + header.payload_len());
        header.write_to(buf);

        // A static R/G gradient with a B channel that scrolls diagonally with the
        // frame index, so motion is obvious at a glance.
        let t = (index & 0xff) as u32;
        for y in 0..self.height {
            let g = (y * 255 / self.height) as u8;
            for x in 0..self.width {
                let r = (x * 255 / self.width) as u8;
                let b = ((x + y + t * 2) & 0xff) as u8;
                buf.extend_from_slice(&[r, g, b, 255]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke() {
        assert_eq!(NAME, "preview-engine");
        // The `preview-engine` → `slang-compile` + `source` edges are real and exercised.
        assert_eq!(slang_compile::NAME, "slang-compile");
        assert_eq!(source::NAME, "source");
    }

    #[test]
    fn gradient_frame_has_header_plus_rgba_payload() {
        let mut src = GradientSource::new(8, 4);
        assert_eq!(src.dimensions(), (8, 4));

        let mut buf = Vec::new();
        src.render_into(3, &mut buf);
        assert_eq!(buf.len(), FRAME_HEADER_LEN + 8 * 4 * 4);
        assert_eq!(&buf[0..4], &FRAME_MAGIC);
        // frame index lands in the header.
        assert_eq!(buf[16], 3);
    }

    #[test]
    fn gradient_animates_between_frames() {
        let mut src = GradientSource::new(16, 16);
        let (mut a, mut b) = (Vec::new(), Vec::new());
        src.render_into(0, &mut a);
        src.render_into(10, &mut b);
        // Same size, different pixels — the frame is actually moving.
        assert_eq!(a.len(), b.len());
        assert_ne!(a[FRAME_HEADER_LEN..], b[FRAME_HEADER_LEN..]);
    }
}

#[cfg(test)]
mod render_tests {
    use super::Renderer;
    use slang_compile::compile_slang;
    use source::Frame;

    // A standard one-pass slang shader (separate texture + sampler, as wgpu's
    // binding model requires). MVP is identity-applied in the VS.
    const PREAMBLE: &str = "\
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
";

    fn passthrough() -> String {
        format!(
            "{PREAMBLE}void main() {{ FragColor = texture(sampler2D(Source, Smp), vTexCoord); }}\n"
        )
    }

    fn invert() -> String {
        format!("{PREAMBLE}void main() {{ vec4 c = texture(sampler2D(Source, Smp), vTexCoord); FragColor = vec4(vec3(1.0) - c.rgb, c.a); }}\n")
    }

    // row0 = [red, green], row1 = [blue, yellow]
    fn checker_2x2() -> Frame {
        Frame::new(
            2,
            2,
            vec![
                255, 0, 0, 255, 0, 255, 0, 255, // top row
                0, 0, 255, 255, 255, 255, 0, 255, // bottom row
            ],
        )
    }

    fn render(shader_src: &str, frame: &Frame) -> Frame {
        let shader = compile_slang(shader_src, None).expect("compile fixture shader");
        let mut r = Renderer::new(frame.width, frame.height).expect("wgpu device");
        r.set_source(frame);
        r.set_shader(&shader);
        r.render().expect("render");
        r.read_back().expect("read back")
    }

    fn close(a: &[u8], b: &[u8]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.abs_diff(*y) <= 2)
    }

    #[test]
    fn passthrough_reproduces_the_image() {
        let input = checker_2x2();
        let output = render(&passthrough(), &input);
        assert_eq!((output.width, output.height), (2, 2));
        assert!(
            close(&output.rgba, &input.rgba),
            "passthrough should reproduce the source\n in: {:?}\nout: {:?}",
            input.rgba,
            output.rgba
        );
    }

    #[test]
    fn non_trivial_shader_transforms_the_image() {
        let input = checker_2x2();
        let output = render(&invert(), &input);
        // Top-left was red (255,0,0) -> inverted to cyan (0,255,255).
        assert!(close(&output.rgba[0..4], &[0, 255, 255, 255]));
        // And the result genuinely differs from the input (not a no-op).
        assert!(!close(&output.rgba, &input.rgba));
    }
}
