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
pub mod pass;
pub mod render_source;
pub mod renderer;
pub mod uniforms;

pub use frame::{FrameHeader, FRAME_HEADER_LEN, FRAME_MAGIC, FRAME_VERSION, PIXEL_FORMAT_RGBA8};
pub use pass::{AxisScale, Pass, ScaleConfig, ScaleType, WrapMode};
pub use render_source::{RenderCommand, RenderSource, DEFAULT_SHADER};
pub use renderer::{Renderer, RendererError, OFFSCREEN_FORMAT};
pub use uniforms::{BuiltinUniforms, BuiltinValues};

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

    // A fixture exercising the full Phase 1 uniform set: the builtin UBO
    // (MVP / *Size / FrameCount) plus a one-parameter Params UBO at binding 3.
    // The UBO member order matches `BuiltinUniforms`' std140 layout.
    const FULL_PREAMBLE: &str = "\
#version 450
#pragma parameter LEVEL \"Level\" 0.5 0.0 1.0 0.01
layout(std140, set = 0, binding = 0) uniform UBO {
    mat4 MVP;
    vec4 SourceSize;
    vec4 OriginalSize;
    vec4 OutputSize;
    uint FrameCount;
} global;
layout(std140, set = 0, binding = 3) uniform Params { float LEVEL; } params;
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
        render_sized(shader_src, frame, (frame.width, frame.height))
    }

    // Render into an output target that may differ from the source size, so the
    // `*Size` uniforms can be told apart.
    fn render_sized(shader_src: &str, frame: &Frame, out: (u32, u32)) -> Frame {
        let shader = compile_slang(shader_src, None).expect("compile fixture shader");
        let mut r = Renderer::new(out.0, out.1).expect("wgpu device");
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

    #[test]
    fn parameter_default_modulates_output() {
        let frag = format!(
            "{FULL_PREAMBLE}void main() {{ vec4 c = texture(sampler2D(Source, Smp), vTexCoord); FragColor = vec4(c.rgb * params.LEVEL, c.a); }}\n"
        );
        let input = checker_2x2();
        let output = render(&frag, &input);
        // Top-left red (255,0,0) scaled by LEVEL's 0.5 default -> (128,0,0).
        assert!(
            close(&output.rgba[0..4], &[128, 0, 0, 255]),
            "got {:?}",
            &output.rgba[0..4]
        );
        // The parameter genuinely dimmed the source (not a passthrough).
        assert!(output.rgba[0] < input.rgba[0]);
    }

    #[test]
    fn builtin_sizes_reach_the_shader() {
        // Output the per-axis Source:Output size ratio so each *Size component is
        // checked independently.
        let frag = format!(
            "{FULL_PREAMBLE}void main() {{ FragColor = vec4(global.SourceSize.x / global.OutputSize.x, global.SourceSize.y / global.OutputSize.y, 0.0, 1.0); }}\n"
        );
        // Source 1x2, output 2x2 -> SourceSize=(1,2), OutputSize=(2,2).
        let input = Frame::new(1, 2, vec![0, 0, 0, 255, 0, 0, 0, 255]);
        let output = render_sized(&frag, &input, (2, 2));
        assert_eq!((output.width, output.height), (2, 2));
        // x = 1/2 -> 128, y = 2/2 -> 255.
        assert!(
            close(&output.rgba[0..4], &[128, 255, 0, 255]),
            "got {:?}",
            &output.rgba[0..4]
        );
    }

    #[test]
    fn frame_count_advances_and_reaches_the_shader() {
        let frag = format!(
            "{FULL_PREAMBLE}void main() {{ FragColor = vec4(float(global.FrameCount) / 255.0, 0.0, 0.0, 1.0); }}\n"
        );
        let shader = compile_slang(&frag, None).expect("compile fixture shader");
        let input = checker_2x2();
        let mut r = Renderer::new(2, 2).expect("wgpu device");
        r.set_source(&input);
        r.set_shader(&shader);
        for _ in 0..20 {
            r.render().expect("render");
        }
        assert_eq!(r.frame_count(), 20);
        let output = r.read_back().expect("read back");
        // The 20th frame was rendered with FrameCount = 19.
        assert!(
            close(&output.rgba[0..4], &[19, 0, 0, 255]),
            "got {:?}",
            &output.rgba[0..4]
        );
    }
}

#[cfg(test)]
mod chain_tests {
    //! Multi-pass chain tests (#22): the N-pass resource graph, the Source
    //! chaining (pass i reads pass i-1's output), and scale-type FBO sizing
    //! surfaced through `OutputSize`.
    use super::{AxisScale, Pass, Renderer, ScaleConfig, ScaleType};
    use slang_compile::{compile_slang, CompiledShader};
    use source::Frame;

    // Standard preamble: builtin UBO (incl. OutputSize) + separate tex/sampler.
    const PREAMBLE: &str = "\
#version 450
layout(std140, set = 0, binding = 0) uniform UBO {
    mat4 MVP;
    vec4 SourceSize;
    vec4 OriginalSize;
    vec4 OutputSize;
    uint FrameCount;
} global;
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

    fn compile(frag_body: &str) -> CompiledShader {
        compile_slang(&format!("{PREAMBLE}{frag_body}"), None).expect("compile chain fixture")
    }

    fn passthrough() -> CompiledShader {
        compile("void main() { FragColor = texture(sampler2D(Source, Smp), vTexCoord); }\n")
    }

    fn invert() -> CompiledShader {
        compile("void main() { vec4 c = texture(sampler2D(Source, Smp), vTexCoord); FragColor = vec4(vec3(1.0) - c.rgb, c.a); }\n")
    }

    fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Frame {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            data.extend_from_slice(&rgba);
        }
        Frame::new(width, height, data)
    }

    fn close(a: &[u8], b: &[u8]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.abs_diff(*y) <= 2)
    }

    fn render_chain(passes: &[Pass], frame: &Frame, out: (u32, u32)) -> Frame {
        let mut r = Renderer::new(out.0, out.1).expect("wgpu device");
        r.set_source(frame);
        r.set_chain(passes).expect("set chain");
        r.render().expect("render");
        r.read_back().expect("read back")
    }

    #[test]
    fn two_pass_invert_invert_is_identity() {
        // invert ∘ invert ≈ identity: the composed chain reproduces the source.
        let input = solid(2, 2, [200, 50, 100, 255]);
        let passes = [Pass::new(invert()), Pass::new(invert())];
        let out = render_chain(&passes, &input, (2, 2));
        assert!(
            close(&out.rgba[0..4], &[200, 50, 100, 255]),
            "invert∘invert should restore the source, got {:?}",
            &out.rgba[0..4]
        );
    }

    #[test]
    fn pass2_reads_pass1_output_not_the_original() {
        // Pass 1 inverts; pass 2 is a passthrough. If pass 2's Source were the
        // ORIGINAL it would reproduce the source; instead it must show pass 1's
        // inverted output. Source red (255,0,0) -> inverted cyan (0,255,255).
        let input = solid(2, 2, [255, 0, 0, 255]);
        let passes = [Pass::new(invert()), Pass::new(passthrough())];
        let out = render_chain(&passes, &input, (2, 2));
        assert!(
            close(&out.rgba[0..4], &[0, 255, 255, 255]),
            "pass 2 should see pass 1's inverted output, got {:?}",
            &out.rgba[0..4]
        );
        assert!(
            !close(&out.rgba[0..4], &input.rgba[0..4]),
            "must NOT reproduce the original (that would mean pass2 read Original)"
        );
    }

    #[test]
    fn single_pass_chain_matches_legacy_behavior() {
        // A 1-pass chain renders straight to the viewport (back-compat path).
        let input = solid(2, 2, [10, 20, 30, 255]);
        let out = render_chain(&[Pass::new(passthrough())], &input, (2, 2));
        assert!(close(&out.rgba[0..4], &input.rgba[0..4]));
    }

    // A fragment that encodes the pass's OWN OutputSize into the color so the
    // resolved FBO size can be read back: R = width/512, G = height/512.
    fn output_size_probe() -> CompiledShader {
        compile("void main() { FragColor = vec4(global.OutputSize.x / 512.0, global.OutputSize.y / 512.0, 0.0, 1.0); }\n")
    }

    /// Render a 2-pass chain whose pass 0 is the intermediate (sized by `scale`)
    /// and whose final pass is a passthrough into the real `viewport`. Pass 0
    /// writes its OutputSize as a uniform color, so the (uniform) final output
    /// decodes the intermediate FBO size. The final viewport is the real one so
    /// `viewport`-scaled intermediates size correctly.
    ///
    /// We decode a **center** pixel, not pixel 0: the §3 v1 default wrap is
    /// `clamp_to_border` (#23), so the corner texel blends with the transparent
    /// border under linear filtering. The output is a uniform color, so any
    /// interior pixel reads the encoded size cleanly.
    fn probe_intermediate_size(
        scale: ScaleConfig,
        src: (u32, u32),
        viewport: (u32, u32),
    ) -> (u32, u32) {
        let input = solid(src.0, src.1, [0, 0, 0, 255]);
        let pass0 = Pass::new(output_size_probe()).with_scale(scale);
        let pass1 = Pass::new(passthrough());
        let out = render_chain(&[pass0, pass1], &input, viewport);
        let center = (((out.height / 2) * out.width + out.width / 2) * 4) as usize;
        let decode = |b: u8| (b as f32 / 255.0 * 512.0).round() as u32;
        (decode(out.rgba[center]), decode(out.rgba[center + 1]))
    }

    fn axis(ty: ScaleType, factor: f32) -> AxisScale {
        AxisScale { ty, factor }
    }

    // The OutputSize probe round-trips through an 8-bit color channel, so decode
    // is exact only to ±~2px at these magnitudes. That tolerance still proves the
    // *scale type* is applied (source/viewport/absolute give very different
    // sizes); the exact rounding/clamping math is unit-tested in `pass.rs`.
    fn size_close(got: (u32, u32), want: (u32, u32)) -> bool {
        got.0.abs_diff(want.0) <= 2 && got.1.abs_diff(want.1) <= 2
    }

    #[test]
    fn source_scale_sizes_intermediate_fbo() {
        // source × 2 of a 100x50 input -> 200x100 intermediate FBO.
        let scale = ScaleConfig {
            x: axis(ScaleType::Source, 2.0),
            y: axis(ScaleType::Source, 2.0),
        };
        let got = probe_intermediate_size(scale, (100, 50), (320, 240));
        assert!(size_close(got, (200, 100)), "source scale: got {got:?}");
    }

    #[test]
    fn viewport_scale_sizes_intermediate_fbo() {
        // viewport × 1 -> the viewport size, independent of the input.
        let scale = ScaleConfig {
            x: axis(ScaleType::Viewport, 1.0),
            y: axis(ScaleType::Viewport, 1.0),
        };
        let got = probe_intermediate_size(scale, (100, 50), (256, 128));
        assert!(size_close(got, (256, 128)), "viewport scale: got {got:?}");
    }

    #[test]
    fn absolute_scale_sizes_intermediate_fbo() {
        // absolute -> literal pixel counts, ignoring input and viewport.
        let scale = ScaleConfig {
            x: axis(ScaleType::Absolute, 300.0),
            y: axis(ScaleType::Absolute, 150.0),
        };
        let got = probe_intermediate_size(scale, (100, 50), (320, 240));
        assert!(size_close(got, (300, 150)), "absolute scale: got {got:?}");
    }

    #[test]
    fn default_intermediate_scale_matches_input() {
        // No scale keys on the intermediate pass -> source × 1.0 (FBO == input).
        let pass0 = Pass::new(output_size_probe());
        let pass1 = Pass::new(passthrough());
        let input = solid(120, 80, [0, 0, 0, 255]);
        let out = render_chain(&[pass0, pass1], &input, (1, 1));
        let decode = |b: u8| (b as f32 / 255.0 * 512.0).round() as u32;
        let got = (decode(out.rgba[0]), decode(out.rgba[1]));
        assert!(size_close(got, (120, 80)), "default scale: got {got:?}");
    }

    #[test]
    fn viewport_change_resizes_viewport_scaled_pass() {
        // A viewport×1 intermediate must reallocate when the viewport changes.
        let scale = ScaleConfig {
            x: AxisScale::VIEWPORT_1X,
            y: AxisScale::VIEWPORT_1X,
        };
        let input = solid(64, 64, [0, 0, 0, 255]);
        let pass0 = Pass::new(output_size_probe()).with_scale(scale);
        let pass1 = Pass::new(passthrough());

        let mut r = Renderer::new(128, 96).expect("wgpu device");
        r.set_source(&input);
        r.set_chain(&[pass0, pass1]).expect("set chain");
        r.set_viewport(1, 1); // shrink to 1x1 so the probe pixel is readable
        r.render().expect("render");
        let out = r.read_back().expect("read back");
        // Now the viewport is 1x1, so the intermediate viewport×1 FBO is 1x1.
        let decode = |b: u8| (b as f32 / 255.0 * 512.0).round() as u32;
        let got = (decode(out.rgba[0]), decode(out.rgba[1]));
        assert!(
            size_close(got, (1, 1)),
            "viewport-change realloc: got {got:?}"
        );
    }
}

#[cfg(test)]
mod format_sampler_tests {
    //! Per-pass FBO format + sampler tests (#23): float/sRGB FBO formats,
    //! `wrap_mode`, `filter_linear`, and `mipmap_input`. All run on the real GPU
    //! and read a region/center pixel back; assertions are tolerant where GPU
    //! filtering varies (the e2e style).
    use super::{Pass, Renderer, ScaleConfig, WrapMode};
    use crate::pass::{AxisScale, ScaleType};
    use slang_compile::{compile_slang, CompiledShader};
    use source::Frame;

    // Standard preamble: builtin UBO + separate tex/sampler (as wgpu requires).
    const PREAMBLE: &str = "\
#version 450
layout(std140, set = 0, binding = 0) uniform UBO {
    mat4 MVP;
    vec4 SourceSize;
    vec4 OriginalSize;
    vec4 OutputSize;
    uint FrameCount;
} global;
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

    fn compile(frag_body: &str) -> CompiledShader {
        compile_slang(&format!("{PREAMBLE}{frag_body}"), None).expect("compile #23 fixture")
    }

    fn passthrough() -> CompiledShader {
        compile("void main() { FragColor = texture(sampler2D(Source, Smp), vTexCoord); }\n")
    }

    /// Pass that ignores its input and writes a constant color (used to seed a
    /// known value into an intermediate FBO regardless of format).
    fn constant(rgba: [f32; 4]) -> CompiledShader {
        compile(&format!(
            "void main() {{ FragColor = vec4({:?}, {:?}, {:?}, {:?}); }}\n",
            rgba[0], rgba[1], rgba[2], rgba[3]
        ))
    }

    fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Frame {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            data.extend_from_slice(&rgba);
        }
        Frame::new(width, height, data)
    }

    fn render_chain(passes: &[Pass], frame: &Frame, out: (u32, u32)) -> Frame {
        let mut r = Renderer::new(out.0, out.1).expect("wgpu device");
        r.set_source(frame);
        r.set_chain(passes).expect("set chain");
        r.render().expect("render");
        r.read_back().expect("read back")
    }

    /// Center-pixel of a read-back frame (avoids border/edge sampling effects).
    fn center(f: &Frame) -> [u8; 4] {
        let i = (((f.height / 2) * f.width + f.width / 2) * 4) as usize;
        [f.rgba[i], f.rgba[i + 1], f.rgba[i + 2], f.rgba[i + 3]]
    }

    fn axis(ty: ScaleType, factor: f32) -> AxisScale {
        AxisScale { ty, factor }
    }

    fn abs_scale(w: f32, h: f32) -> ScaleConfig {
        ScaleConfig {
            x: axis(ScaleType::Absolute, w),
            y: axis(ScaleType::Absolute, h),
        }
    }

    // ---- 1a. Float framebuffer retains range an 8-bit FBO would clip. ----

    /// Pass 0 writes `2.0` (R) into its FBO; pass 1 multiplies by `0.25` and reads
    /// it back. A float16 FBO stores 2.0, so pass 1 yields `0.5` (~128). An 8-bit
    /// UNORM FBO clips pass 0's 2.0 to 1.0, so pass 1 would yield `0.25` (~64).
    fn float_range_value(float_fbo: bool) -> u8 {
        let mut p0 = Pass::new(constant([2.0, 0.0, 0.0, 1.0])).with_scale(abs_scale(8.0, 8.0));
        p0.float_framebuffer = float_fbo;
        // Pass 1 reads the FBO at the (uniform) center and scales by 0.25.
        let p1 = compile(
            "void main() { vec4 c = texture(sampler2D(Source, Smp), vTexCoord); FragColor = vec4(c.rgb * 0.25, 1.0); }\n",
        );
        let out = render_chain(&[p0, Pass::new(p1)], &solid(8, 8, [0, 0, 0, 255]), (8, 8));
        center(&out)[0]
    }

    #[test]
    fn float_framebuffer_preserves_values_above_one() {
        let with_float = float_range_value(true);
        let with_unorm = float_range_value(false);
        // float16: 2.0 * 0.25 = 0.5 -> ~128. unorm: clipped 1.0 * 0.25 = 0.25 -> ~64.
        assert!(
            (with_float as i32 - 128).abs() <= 16,
            "float16 FBO should retain 2.0 (expect ~128), got {with_float}"
        );
        assert!(
            (with_unorm as i32 - 64).abs() <= 16,
            "8-bit FBO should clip 2.0->1.0 (expect ~64), got {with_unorm}"
        );
        assert!(
            with_float > with_unorm + 30,
            "float16 must beat the 8-bit clip: float {with_float} vs unorm {with_unorm}"
        );
    }

    // ---- 1b. sRGB FBO round-trip: linear value survives encode+decode. ----

    /// Pass 0 writes a known **linear** value into an sRGB FBO (HW encodes
    /// linear->sRGB on store); pass 1 samples it (HW decodes sRGB->linear) into the
    /// 8-bit *linear* offscreen. The two conversions are inverses, so pass 1 reads
    /// back the original linear value — not a double-applied gamma.
    #[test]
    fn srgb_framebuffer_round_trips_linear_value() {
        // 0.5 linear. Through an sRGB FBO (encode then decode) it must come back
        // ~0.5 (~128). If sRGB were wrongly skipped on one side, 0.5 linear would
        // read back as ~0.735 (~188, the sRGB-encoded value) or ~0.214 (~55).
        let mut p0 = Pass::new(constant([0.5, 0.5, 0.5, 1.0])).with_scale(abs_scale(8.0, 8.0));
        p0.srgb_framebuffer = true;
        let out = render_chain(
            &[p0, Pass::new(passthrough())],
            &solid(8, 8, [0, 0, 0, 255]),
            (8, 8),
        );
        let got = center(&out)[0];
        assert!(
            (got as i32 - 128).abs() <= 12,
            "sRGB round-trip of linear 0.5 should read ~128, got {got} \
             (a value ~188 or ~55 would mean the linear<->sRGB conversion is wrong)"
        );
    }

    // ---- 2. Wrap modes at out-of-range UVs. ----

    /// A red horizontal ramp (R = u) is written into pass 0's FBO; the final pass
    /// samples it at a **fixed out-of-range UV** and outputs that color. The wrap
    /// mode (set on the sampling pass) determines what UV 1.25 resolves to.
    fn wrap_sample_r(wrap: WrapMode) -> u8 {
        // Pass 0: write R = vTexCoord.x (0 at left -> 1 at right).
        let ramp = compile("void main() { FragColor = vec4(vTexCoord.x, 0.0, 0.0, 1.0); }\n");
        let p0 = Pass::new(ramp).with_scale(abs_scale(64.0, 8.0));
        // Pass 1: sample at u = 1.25 (out of [0,1]); v = 0.5 (mid).
        let probe = compile(
            "void main() { FragColor = texture(sampler2D(Source, Smp), vec2(1.25, 0.5)); }\n",
        );
        let mut p1 = Pass::new(probe);
        p1.wrap_mode = wrap;
        p1.filter_linear = true;
        let out = render_chain(&[p0, p1], &solid(8, 8, [0, 0, 0, 255]), (16, 16));
        center(&out)[0]
    }

    #[test]
    fn wrap_repeat_vs_clamp_to_edge() {
        // Repeat: 1.25 -> 0.25 -> R ~= 0.25 (~64).
        let repeat = wrap_sample_r(WrapMode::Repeat);
        // ClampToEdge: 1.25 -> 1.0 -> R ~= 1.0 (~255).
        let clamp = wrap_sample_r(WrapMode::ClampToEdge);
        assert!(
            (repeat as i32 - 64).abs() <= 30,
            "repeat at u=1.25 should sample ~0.25 (~64), got {repeat}"
        );
        assert!(
            clamp >= 220,
            "clamp_to_edge at u=1.25 should sample the right edge (~255), got {clamp}"
        );
    }

    #[test]
    fn wrap_mirrored_repeat() {
        // MirroredRepeat: 1.25 -> mirror -> 0.75 -> R ~= 0.75 (~191).
        let mirror = wrap_sample_r(WrapMode::MirroredRepeat);
        assert!(
            (mirror as i32 - 191).abs() <= 30,
            "mirrored_repeat at u=1.25 should sample ~0.75 (~191), got {mirror}"
        );
    }

    #[test]
    fn wrap_clamp_to_border_or_fallback() {
        // Build a renderer just to query feature support for the assertion.
        let supported = Renderer::new(8, 8)
            .expect("wgpu device")
            .clamp_to_border_supported();
        let border = wrap_sample_r(WrapMode::ClampToBorder);
        if supported {
            // Transparent-black border: R ~= 0 at the out-of-range UV.
            assert!(
                border <= 30,
                "clamp_to_border at u=1.25 should sample the (black) border, got {border}"
            );
        } else {
            // Fallback to clamp_to_edge: samples the right edge (~255). Don't
            // hard-fail on CI lacking the feature; just prove it didn't crash and
            // gave the documented edge fallback.
            assert!(
                border >= 220,
                "clamp_to_border fallback (no feature) should clamp to edge (~255), got {border}"
            );
        }
    }

    // ---- 3. filter_linear: nearest vs linear at a sub-texel coordinate. ----

    /// Sample a 2x1 black/white source at u=0.5 (between the two texel centers).
    /// Linear -> the average (~127); nearest -> exactly one texel (0 or 255), never
    /// the average. The pass doing the sampling is pass 0 (a single pass).
    fn filter_sample_at_half(filter_linear: bool) -> u8 {
        // 2x1: texel0 black, texel1 white.
        let src = Frame::new(2, 1, vec![0, 0, 0, 255, 255, 255, 255, 255]);
        // Sample at the exact midpoint u=0.5.
        let probe = compile(
            "void main() { FragColor = texture(sampler2D(Source, Smp), vec2(0.5, 0.5)); }\n",
        );
        let mut p = Pass::new(probe);
        p.filter_linear = filter_linear;
        p.wrap_mode = WrapMode::ClampToEdge;
        let out = render_chain(&[p], &src, (8, 8));
        center(&out)[0]
    }

    #[test]
    fn filter_linear_vs_nearest_at_subtexel() {
        let linear = filter_sample_at_half(true);
        let nearest = filter_sample_at_half(false);
        // Linear blends the two texels -> ~127.
        assert!(
            (linear as i32 - 127).abs() <= 30,
            "linear at u=0.5 should be the ~average (~127), got {linear}"
        );
        // Nearest picks one texel -> near 0 or near 255, never the average.
        assert!(
            nearest <= 60 || nearest >= 195,
            "nearest at u=0.5 should be a single texel (0 or 255), got {nearest}"
        );
    }

    // ---- 4. mipmap_input: a coarse mip of a 2-color FBO is ~the average. ----

    /// Pass 0 writes a sharp left=red / right=green split into a 64x64 FBO; pass 1
    /// reads it with `mipmap_input` and samples a **coarse LOD** (so the two halves
    /// average toward ~yellow). Without `mipmap_input` the FBO has no mips and the
    /// coarse-LOD request resolves to the base level (a sharp split, not the avg).
    fn mip_coarse_sample(mipmap_input: bool) -> [u8; 4] {
        // Left half red, right half green, written by texel position.
        let split = compile(
            "void main() { FragColor = vTexCoord.x < 0.5 ? vec4(1.0,0.0,0.0,1.0) : vec4(0.0,1.0,0.0,1.0); }\n",
        );
        let p0 = Pass::new(split).with_scale(abs_scale(64.0, 64.0));
        // Pass 1 samples the whole texture at a high LOD via textureLod(...).
        let probe = compile(
            "void main() { FragColor = textureLod(sampler2D(Source, Smp), vec2(0.5, 0.5), 6.0); }\n",
        );
        let mut p1 = Pass::new(probe);
        p1.mipmap_input = mipmap_input;
        // Nearest mag/min so the base level (no-mips case) reads a single texel of
        // the sharp split; the mip chain itself is still built with a linear
        // downsample, so the coarse LOD is the averaged value either way.
        p1.filter_linear = false;
        let out = render_chain(&[p0, p1], &solid(8, 8, [0, 0, 0, 255]), (8, 8));
        center(&out)
    }

    #[test]
    fn mipmap_input_generates_sampled_mips() {
        // With mips: LOD 6 of a 64x64 split averages red+green -> both channels mid
        // (~yellow). The coarse mip is the box-down average, so R and G are similar.
        let with_mips = mip_coarse_sample(true);
        assert!(
            with_mips[0] > 60 && with_mips[1] > 60,
            "mipmap_input: coarse LOD should average red+green (both channels lit), got {with_mips:?}"
        );
        assert!(
            (with_mips[0] as i32 - with_mips[1] as i32).abs() <= 60,
            "mipmap_input: averaged mip should have R~=G, got {with_mips:?}"
        );

        // Without mips: no mip chain exists, so the LOD-6 request clamps to the
        // base level — a single texel of the sharp split (red OR green), NOT the
        // red+green average.
        let no_mips = mip_coarse_sample(false);
        let both_lit = no_mips[0] > 60 && no_mips[1] > 60;
        assert!(
            !both_lit,
            "without mipmap_input the base level is a sharp split (one channel), got {no_mips:?}"
        );
    }
}
