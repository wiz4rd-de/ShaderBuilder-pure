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

pub mod bindtable;
pub mod frame;
pub mod pass;
pub mod render_source;
pub mod renderer;
pub mod uniforms;
pub mod viewport;

pub use bindtable::{PlaceholderResolver, TextureClass, TextureResolver};
pub use frame::{FrameHeader, FRAME_HEADER_LEN, FRAME_MAGIC, FRAME_VERSION, PIXEL_FORMAT_RGBA8};
pub use pass::{AxisScale, Pass, ScaleConfig, ScaleType, WrapMode};
pub use render_source::{RenderCommand, RenderSource, DEFAULT_SHADER};
pub use renderer::{LutSpec, Renderer, RendererError, OFFSCREEN_FORMAT};
pub use uniforms::{BuiltinUniforms, BuiltinValues, ParamDef, ParamStore, ParamView};
pub use viewport::{ViewportConfig, ViewportRect};

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
    fn final_pass_with_explicit_scale_renders_scaled_then_stretches() {
        // #22 §2/§10: a LAST pass declaring an explicit `scale` must render into
        // its OWN FBO sized by that scale (receiving OutputSize == that size) and
        // THEN be stretched into the viewport-sized offscreen target.
        //
        // The final pass is the OutputSize probe with an absolute 64x64 scale, in a
        // 256x256 viewport. Its FBO is 64x64, so OutputSize == (64,64). The probe
        // writes that encoded size as a UNIFORM color into the 64x64 FBO, which is
        // then stretched to 256x256 — so:
        //   (1) decoding any read-back pixel yields OutputSize == 64x64 (NOT 256),
        //       proving the final shader saw its scaled FBO size; and
        //   (2) the read-back frame is the full viewport size (256x256), proving
        //       the stretch/blit actually ran.
        let viewport = (256, 256);
        let scale = ScaleConfig {
            x: axis(ScaleType::Absolute, 64.0),
            y: axis(ScaleType::Absolute, 64.0),
        };
        let input = solid(100, 50, [0, 0, 0, 255]);
        let final_pass = Pass::new(output_size_probe()).with_scale(scale);
        let out = render_chain(&[final_pass], &input, viewport);

        // (2) The read-back is at the viewport size (the stretch target).
        assert_eq!(
            (out.width, out.height),
            viewport,
            "read-back must be the viewport size (stretched)"
        );
        // (1) Decode OutputSize from the (uniform, stretched) result — it is the
        // SCALED FBO size, not the viewport.
        let center = (((out.height / 2) * out.width + out.width / 2) * 4) as usize;
        let decode = |b: u8| (b as f32 / 255.0 * 512.0).round() as u32;
        let got = (decode(out.rgba[center]), decode(out.rgba[center + 1]));
        assert!(
            size_close(got, (64, 64)),
            "final pass must see OutputSize == its scaled FBO (64x64), not the \
             viewport (256); got {got:?}"
        );
    }

    #[test]
    fn final_pass_explicit_downscale_stretches_a_distinguishable_pattern() {
        // A two-pass chain whose FINAL pass downscales to a tiny FBO then stretches
        // to the viewport. Pass 0 writes a left=red / right=green split into a wide
        // intermediate; the final pass downsamples it to an absolute 2x2 FBO with a
        // linear sampler (averaging) and is stretched up. The left-edge and
        // right-edge of the viewport must still read red-ish vs green-ish — proving
        // the scaled FBO content (not a viewport-sized direct render) was stretched.
        let split = compile(
            "void main() { FragColor = vTexCoord.x < 0.5 ? vec4(1.0,0.0,0.0,1.0) : vec4(0.0,1.0,0.0,1.0); }\n",
        );
        let pass0 = Pass::new(split).with_scale(ScaleConfig {
            x: axis(ScaleType::Absolute, 64.0),
            y: axis(ScaleType::Absolute, 8.0),
        });
        // Final pass: a passthrough downscaled to an absolute 4x4 FBO, then
        // stretched to the 64x16 viewport.
        let final_pass = Pass::new(passthrough()).with_scale(ScaleConfig {
            x: axis(ScaleType::Absolute, 4.0),
            y: axis(ScaleType::Absolute, 4.0),
        });
        let out = render_chain(&[pass0, final_pass], &solid(8, 8, [0, 0, 0, 255]), (64, 16));
        assert_eq!((out.width, out.height), (64, 16), "stretched to viewport");
        // Left quarter is red-dominant; right quarter is green-dominant.
        let px = |x: u32, y: u32| {
            let i = ((y * out.width + x) * 4) as usize;
            [out.rgba[i], out.rgba[i + 1]]
        };
        let left = px(8, 8);
        let right = px(56, 8);
        assert!(
            left[0] > left[1],
            "left edge should stay red-dominant, got {left:?}"
        );
        assert!(
            right[1] > right[0],
            "right edge should stay green-dominant, got {right:?}"
        );
    }

    #[test]
    fn viewport_change_resizes_viewport_scaled_pass() {
        // A viewport×1 intermediate must reallocate when the viewport changes
        // (stale `f.size != size` path). Render ONCE at the initial viewport and
        // assert the intermediate FBO is sized to it, THEN change the viewport and
        // assert the intermediate reallocates — so a missing realloc would fail the
        // SECOND assertion (the first proves the initial size was real, not 1x1).
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

        // First render at 128x96: the viewport×1 intermediate FBO is 128 wide.
        r.render().expect("render");
        let out0 = r.read_back().expect("read back");
        let center0 = (((out0.height / 2) * out0.width + out0.width / 2) * 4) as usize;
        let decode = |b: u8| (b as f32 / 255.0 * 512.0).round() as u32;
        let got0 = (decode(out0.rgba[center0]), decode(out0.rgba[center0 + 1]));
        assert!(
            size_close(got0, (128, 96)),
            "initial viewport×1 FBO should be 128x96, got {got0:?}"
        );

        // Change the viewport to 1x1 and render again: the intermediate viewport×1
        // FBO must reallocate to 1x1 (the realloc-on-change path).
        r.set_viewport(1, 1);
        r.render().expect("render");
        let out1 = r.read_back().expect("read back");
        let got1 = (decode(out1.rgba[0]), decode(out1.rgba[1]));
        assert!(
            size_close(got1, (1, 1)),
            "viewport-change realloc: FBO should become 1x1, got {got1:?}"
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

    // ---- 1b. sRGB FBO actually selects the sRGB 8-bit format. ----

    /// Render the SAME non-midpoint linear value (0.2) through an sRGB-FBO
    /// intermediate vs a plain-UNORM-FBO intermediate and assert the two read-backs
    /// DIFFER (#23/G). An sRGB FBO stores the linear value *encoded* (8-bit), so it
    /// quantizes onto a different bucket than a plain UNORM FBO; pass 1 then samples
    /// (HW decodes sRGB->linear) into the linear offscreen. The 8-bit round-trip
    /// difference is tiny on its own, so pass 1 **amplifies** the gap from 0.2 by
    /// 20x to lift it well clear of GPU rounding (~6 bytes apart, reproducibly).
    ///
    /// This proves `fbo_format()` genuinely selects `Rgba8UnormSrgb` for the sRGB
    /// pass: the OLD test wrote 0.5 (the one value where sRGB and UNORM round-trips
    /// coincide at ~128) and so could not tell an sRGB FBO from a UNORM one.
    fn srgb_vs_unorm_amplified(srgb: bool) -> u8 {
        // Pass 1 amplifies the sampled value's deviation from 0.2 by 20x.
        let amp = compile(
            "void main() { float c = texture(sampler2D(Source, Smp), vTexCoord).r; FragColor = vec4(clamp((c - 0.2) * 20.0 + 0.5, 0.0, 1.0), 0.0, 0.0, 1.0); }\n",
        );
        let mut p0 = Pass::new(constant([0.2, 0.0, 0.0, 1.0])).with_scale(abs_scale(8.0, 8.0));
        p0.srgb_framebuffer = srgb;
        center(&render_chain(
            &[p0, Pass::new(amp)],
            &solid(8, 8, [0, 0, 0, 255]),
            (8, 8),
        ))[0]
    }

    #[test]
    fn srgb_framebuffer_diverges_from_unorm_at_non_midpoint() {
        let srgb = srgb_vs_unorm_amplified(true) as i32;
        let unorm = srgb_vs_unorm_amplified(false) as i32;
        // The plain UNORM FBO stores 0.2 exactly (51) -> amplified value is ~127.
        // The sRGB FBO stores the *encoded* 0.2 and decodes to a slightly different
        // linear value -> amplified value lands clearly apart (~6 bytes). If both
        // were UNORM (the bug this guards) the two would be identical.
        assert!(
            (srgb - unorm).abs() >= 3,
            "sRGB FBO must store/round-trip differently from a plain UNORM FBO \
             (proving Rgba8UnormSrgb is actually selected); got srgb={srgb} unorm={unorm}"
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

    // ---- 4b. mipmap_input on PASS 0 mips the SOURCE texture (#23/F). ----

    /// A single pass reads the SOURCE with `mipmap_input` and samples a coarse LOD.
    /// The source is a 64x64 left=red / right=green split, so a coarse mip averages
    /// toward yellow. With `mipmap_input0` the source must carry a generated mip
    /// chain; without it the source is single-level and the LOD request clamps to
    /// the (sharp-split) base level.
    fn source_mip_coarse_sample(mipmap_input0: bool) -> [u8; 4] {
        // 64x64 source: left half red, right half green.
        let mut data = Vec::with_capacity(64 * 64 * 4);
        for _ in 0..64 {
            for x in 0..64u32 {
                if x < 32 {
                    data.extend_from_slice(&[255, 0, 0, 255]);
                } else {
                    data.extend_from_slice(&[0, 255, 0, 255]);
                }
            }
        }
        let src = Frame::new(64, 64, data);
        // Pass 0 samples the source itself at a high LOD.
        let probe = compile(
            "void main() { FragColor = textureLod(sampler2D(Source, Smp), vec2(0.5, 0.5), 6.0); }\n",
        );
        let mut p0 = Pass::new(probe);
        p0.mipmap_input = mipmap_input0;
        // Nearest so the no-mips base read is a single (sharp-split) texel; the mip
        // chain itself is built with a linear downsample, so the coarse LOD averages.
        p0.filter_linear = false;
        let out = render_chain(&[p0], &src, (8, 8));
        center(&out)
    }

    #[test]
    fn mipmap_input_on_pass0_mips_the_source() {
        // With mipmap_input0: the source gets a real mip chain, so LOD 6 averages
        // red+green (both channels lit, R~=G).
        let with_mips = source_mip_coarse_sample(true);
        assert!(
            with_mips[0] > 60 && with_mips[1] > 60,
            "mipmap_input0: coarse LOD should average red+green, got {with_mips:?}"
        );
        assert!(
            (with_mips[0] as i32 - with_mips[1] as i32).abs() <= 60,
            "mipmap_input0: averaged source mip should have R~=G, got {with_mips:?}"
        );

        // Without it: the source is single-level, so LOD 6 clamps to the base level
        // — a single texel of the sharp split (one channel), NOT the average.
        let no_mips = source_mip_coarse_sample(false);
        assert!(
            !(no_mips[0] > 60 && no_mips[1] > 60),
            "without mipmap_input0 the source base level is a sharp split, got {no_mips:?}"
        );
    }
}

#[cfg(test)]
mod parameter_tests {
    //! Reflection-driven parameter packing + live `set_parameter` tests (#29).
    //! Real GPU: each reads a center pixel back to prove a `#pragma parameter`
    //! reaches the shader at its reflected offset, that a live update lands within
    //! one frame WITHOUT recompiling, that preset overrides set the initial value,
    //! that a parameter is global-by-name across passes, and that sets clamp.
    use super::{ParamStore, Pass, Renderer};
    use slang_compile::{compile_slang, CompiledShader};
    use source::Frame;

    /// Vertex stage shared by the fixtures (applies MVP, forwards TexCoord).
    const VS: &str = "\
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

    fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Frame {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            data.extend_from_slice(&rgba);
        }
        Frame::new(width, height, data)
    }

    fn center(f: &Frame) -> [u8; 4] {
        let i = (((f.height / 2) * f.width + f.width / 2) * 4) as usize;
        [f.rgba[i], f.rgba[i + 1], f.rgba[i + 2], f.rgba[i + 3]]
    }

    /// A pass with a dedicated `Params` UBO (binding 3) declaring two params in
    /// NON-canonical order (B before A), plus a builtin UBO with a builtin AND a
    /// param (LEVEL) mixed in — exercising both the dedicated-block and the
    /// mixed-block packing paths in one shader. Output: R=A, G=B, B=LEVEL.
    fn mixed_param_shader() -> CompiledShader {
        let src = format!(
            "#version 450
#pragma parameter A \"A\" 0.25 0.0 1.0 0.01
#pragma parameter B \"B\" 0.75 0.0 1.0 0.01
#pragma parameter LEVEL \"Level\" 0.5 0.0 1.0 0.01
layout(std140, set = 0, binding = 0) uniform UBO {{ mat4 MVP; vec4 OutputSize; float LEVEL; }} global;
layout(std140, set = 0, binding = 3) uniform Params {{ float B; float A; }} params;
{VS}void main() {{ FragColor = vec4(params.A, params.B, global.LEVEL, 1.0); }}
"
        );
        compile_slang(&src, None).expect("compile mixed param shader")
    }

    /// Param-only fixture: a single param X scaling a constant red. Output R = X.
    fn x_scale_shader() -> CompiledShader {
        let src = format!(
            "#version 450
#pragma parameter X \"X\" 0.5 0.0 1.0 0.01
layout(std140, set = 0, binding = 0) uniform UBO {{ mat4 MVP; }} global;
layout(std140, set = 0, binding = 3) uniform Params {{ float X; }} params;
{VS}void main() {{ FragColor = vec4(params.X, 0.0, 0.0, 1.0); }}
"
        );
        compile_slang(&src, None).expect("compile x-scale shader")
    }

    #[test]
    fn param_defaults_reach_shader_at_reflected_offsets() {
        // Defaults A=0.25 (~64), B=0.75 (~191), LEVEL=0.5 (~128). The Params UBO
        // declares B before A, and LEVEL lives in the builtin block — if packing
        // were positional rather than offset-by-name, these would be swapped.
        let mut r = Renderer::new(8, 8).expect("wgpu device");
        r.set_source(&solid(8, 8, [0, 0, 0, 255]));
        r.set_shader(&mixed_param_shader());
        r.render().expect("render");
        let c = center(&r.read_back().expect("read back"));
        assert!((c[0] as i32 - 64).abs() <= 3, "A=0.25 -> R~64, got {c:?}");
        assert!((c[1] as i32 - 191).abs() <= 3, "B=0.75 -> G~191, got {c:?}");
        assert!(
            (c[2] as i32 - 128).abs() <= 3,
            "LEVEL=0.5 -> B~128, got {c:?}"
        );
    }

    #[test]
    fn set_parameter_updates_live_without_recompile() {
        let shader = x_scale_shader();
        let mut r = Renderer::new(8, 8).expect("wgpu device");
        r.set_source(&solid(8, 8, [0, 0, 0, 255]));
        // set_shader compiles + builds the pipeline exactly once, here.
        r.set_shader(&shader);
        r.render().expect("render");
        let before = center(&r.read_back().expect("read back"))[0];
        assert!((before as i32 - 128).abs() <= 3, "default X=0.5 -> ~128");

        // Change X live. This must NOT recompile or rebuild the pipeline — just
        // re-pack + re-upload the param UBO on the next frame.
        assert!(r.set_parameter("X", 0.25));
        r.render().expect("render");
        let after = center(&r.read_back().expect("read back"))[0];
        assert!(
            (after as i32 - 64).abs() <= 3,
            "X=0.25 -> ~64 after live set, got {after}"
        );
        assert!(
            after < before,
            "the live update genuinely changed the output"
        );
    }

    #[test]
    fn set_parameter_does_not_call_compile() {
        // Prove no recompile by reusing the SAME CompiledShader: if set_parameter
        // rebuilt from source it would need to re-run the toolchain. We assert the
        // engine reflects the change purely from the live store. (compile_slang is
        // only ever invoked here in the test setup, once.)
        let shader = x_scale_shader();
        let mut r = Renderer::new(8, 8).expect("wgpu device");
        r.set_source(&solid(8, 8, [0, 0, 0, 255]));
        r.set_shader(&shader);
        for v in [0.1f32, 0.4, 0.9] {
            assert!(r.set_parameter("X", v));
            r.render().expect("render");
            let got = center(&r.read_back().expect("read back"))[0];
            let want = (v * 255.0).round() as i32;
            assert!(
                (got as i32 - want).abs() <= 4,
                "X={v} -> ~{want}, got {got}"
            );
        }
    }

    #[test]
    fn preset_override_sets_the_initial_value() {
        let shader = x_scale_shader();
        let mut r = Renderer::new(8, 8).expect("wgpu device");
        r.set_source(&solid(8, 8, [0, 0, 0, 255]));
        r.set_shader(&shader);

        // Build the param store from the collected params, then layer a preset
        // override (X -> 0.8) — exactly what the app does on preset load.
        let mut store = r.collected_params().clone();
        let mut overrides = std::collections::BTreeMap::new();
        overrides.insert("X".to_string(), 0.8);
        store.apply_overrides(&overrides);
        r.set_params(store);

        r.render().expect("render");
        let got = center(&r.read_back().expect("read back"))[0];
        // 0.8, not the pragma default 0.5.
        assert!(
            (got as i32 - 204).abs() <= 4,
            "override X=0.8 -> ~204, got {got}"
        );
    }

    #[test]
    fn parameter_is_global_by_name_across_passes() {
        // Two passes both declaring X. Pass 0 writes X into its FBO (R=X); pass 1
        // multiplies its input by X again (R = X * input.R = X*X). A single
        // set_parameter("X", v) must drive BOTH passes.
        let writer = {
            let src = format!(
                "#version 450
#pragma parameter X \"X\" 0.5 0.0 1.0 0.01
layout(std140, set = 0, binding = 0) uniform UBO {{ mat4 MVP; }} global;
layout(std140, set = 0, binding = 3) uniform Params {{ float X; }} params;
{VS}void main() {{ FragColor = vec4(params.X, 0.0, 0.0, 1.0); }}
"
            );
            compile_slang(&src, None).expect("compile writer")
        };
        let multiplier = {
            let src = format!(
                "#version 450
#pragma parameter X \"X\" 0.5 0.0 1.0 0.01
layout(std140, set = 0, binding = 0) uniform UBO {{ mat4 MVP; }} global;
layout(std140, set = 0, binding = 3) uniform Params {{ float X; }} params;
{VS}void main() {{ vec4 c = texture(sampler2D(Source, Smp), vTexCoord); FragColor = vec4(c.r * params.X, 0.0, 0.0, 1.0); }}
"
            );
            compile_slang(&src, None).expect("compile multiplier")
        };
        let mut r = Renderer::new(8, 8).expect("wgpu device");
        r.set_source(&solid(8, 8, [0, 0, 0, 255]));
        r.set_chain(&[Pass::new(writer), Pass::new(multiplier)])
            .expect("set chain");
        // X collected once (global by name).
        assert_eq!(r.parameters().len(), 1, "X deduped across passes");

        // Set X to a NON-default value (default is 0.5). With X=0.6 the cross-pass
        // product is 0.6*0.6=0.36 (~92). This is robust against false greens:
        // - a no-op set would leave both at the default 0.5 -> 0.25 (~64);
        // - a world where only one pass picks up X (the other defaulting to 0.5)
        //   would give 0.6*0.5=0.30 (~76).
        // Only X reaching BOTH passes at the set value yields ~92.
        assert!(r.set_parameter("X", 0.6));
        r.render().expect("render");
        let got = center(&r.read_back().expect("read back"))[0];
        assert!(
            (got as i32 - 92).abs() <= 5,
            "X=0.6 applied in BOTH passes -> 0.6*0.6=0.36 (~92), got {got} \
             (a no-op set gives ~64; one-pass-only gives ~76)"
        );
    }

    #[test]
    fn set_parameter_clamps_the_rendered_pixel_but_surfaces_raw() {
        // §11 item 7: the store keeps the RAW value (surfaced to the UI), but the
        // clamp is applied at use so the rendered pixel stays in range.
        let shader = x_scale_shader(); // X in [0,1]
        let mut r = Renderer::new(8, 8).expect("wgpu device");
        r.set_source(&solid(8, 8, [0, 0, 0, 255]));
        r.set_shader(&shader);

        // Beyond max: the rendered pixel clamps to 1.0 -> ~255.
        assert!(r.set_parameter("X", 9.0));
        r.render().expect("render");
        assert!(
            center(&r.read_back().expect("read back"))[0] >= 250,
            "clamp to max"
        );

        // Below min: the rendered pixel clamps to 0.0 -> ~0.
        assert!(r.set_parameter("X", -9.0));
        r.render().expect("render");
        assert!(
            center(&r.read_back().expect("read back"))[0] <= 5,
            "clamp to min"
        );

        // The view surfaces the RAW value the user set (-9.0), NOT the clamped one
        // — the clamp lives at packing time, not in the store (§11 item 7).
        let v = r.parameters().into_iter().find(|p| p.name == "X").unwrap();
        assert_eq!(v.value, -9.0, "raw value surfaced to the UI");
    }

    #[test]
    fn builtin_wins_over_a_param_colliding_with_its_name() {
        // A `#pragma parameter OutputSize` collides with the builtin OutputSize
        // semantic. RetroArch resolves this in the builtin's favor: the shader
        // must see the real OutputSize (the pane width), NOT the param default
        // (#28/#29). The param's small default would read back near 0; the
        // builtin's width (the pane) reads back near 200.
        let src = format!(
            "#version 450
#pragma parameter OutputSize \"Bogus\" 0.0 0.0 1.0 0.01
layout(std140, set = 0, binding = 0) uniform UBO {{ mat4 MVP; vec4 OutputSize; }} global;
{VS}void main() {{ FragColor = vec4(global.OutputSize.x / 255.0, 0.0, 0.0, 1.0); }}
"
        );
        let shader = compile_slang(&src, None).expect("compile collide shader");
        // Pane = 200 wide so the builtin OutputSize.x is 200 (~200 read back).
        let mut r = Renderer::new(200, 64).expect("wgpu device");
        r.set_source(&solid(8, 8, [0, 0, 0, 255]));
        r.set_shader(&shader);
        r.render().expect("render");
        let got = center(&r.read_back().expect("read back"))[0];
        assert!(
            (got as i32 - 200).abs() <= 3,
            "builtin OutputSize (200) must reach the shader, not the param default (0); got {got}"
        );
    }

    #[test]
    fn collected_store_seeds_defaults_for_a_chain() {
        // collected_params() returns defaults before any override/set.
        let mut r = Renderer::new(8, 8).expect("wgpu device");
        r.set_source(&solid(8, 8, [0, 0, 0, 255]));
        r.set_shader(&mixed_param_shader());
        let store: ParamStore = r.collected_params().clone();
        let views = store.views();
        assert_eq!(views.len(), 3, "A, B, LEVEL collected");
        let a = views.iter().find(|v| v.name == "A").unwrap();
        assert_eq!(a.value, 0.25, "default seeded");
    }
}

#[cfg(test)]
mod builtin_semantics_tests {
    //! Full builtin-semantics + reflection-driven-packing tests (#28). These run
    //! on the real GPU and read a center pixel back; they prove the renderer
    //! packs every computable semantic at the *reflected* offset (not a fixed
    //! layout), applies per-pass `frame_count_mod`, and threads the size family
    //! (`OutputSize`, `FinalViewportSize`, `PassNSize`) through a chain.
    use super::{Pass, Renderer};
    use slang_compile::{compile_slang, CompiledShader};
    use source::Frame;

    /// A vertex stage that just applies MVP (kept identical across fixtures so
    /// only the fragment body / UBO declaration varies).
    const VS: &str = "\
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

    fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Frame {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            data.extend_from_slice(&rgba);
        }
        Frame::new(width, height, data)
    }

    /// Center pixel of a read-back frame (avoids border/edge effects).
    fn center(f: &Frame) -> [u8; 4] {
        let i = (((f.height / 2) * f.width + f.width / 2) * 4) as usize;
        [f.rgba[i], f.rgba[i + 1], f.rgba[i + 2], f.rgba[i + 3]]
    }

    fn render_chain(passes: &[Pass], frame: &Frame, out: (u32, u32)) -> Frame {
        let mut r = Renderer::new(out.0, out.1).expect("wgpu device");
        r.set_source(frame);
        r.set_chain(passes).expect("set chain");
        r.render().expect("render");
        r.read_back().expect("read back")
    }

    /// Reflection-driven proof: declare the builtin UBO members in a
    /// NON-canonical order AND as a subset (no SourceSize/OriginalSize), and read
    /// back a value derived from each declared semantic. If packing were
    /// fixed-layout rather than offset-by-name, the reordered members would carry
    /// the wrong bytes and the assertions would fail.
    #[test]
    fn non_canonical_subset_builtins_each_reach_the_shader() {
        // Members deliberately scrambled: FrameCount, then OutputSize, then MVP.
        let src = format!(
            "#version 450
layout(std140, set = 0, binding = 0) uniform UBO {{
    uint FrameCount;
    vec4 OutputSize;
    vec4 FinalViewportSize;
    mat4 MVP;
}} global;
{VS}void main() {{
    // R = OutputSize.x/255, G = FinalViewportSize.x/255, B = FrameCount/255.
    FragColor = vec4(
        global.OutputSize.x / 255.0,
        global.FinalViewportSize.x / 255.0,
        float(global.FrameCount) / 255.0,
        1.0);
}}
"
        );
        let shader = compile_slang(&src, None).expect("compile");
        // Output pane = 200 wide; render 6 frames so FrameCount = 5 on the last.
        let mut r = Renderer::new(200, 100).expect("wgpu device");
        r.set_source(&solid(8, 8, [0, 0, 0, 255]));
        r.set_shader(&shader);
        for _ in 0..6 {
            r.render().expect("render");
        }
        let out = r.read_back().expect("read back");
        let c = center(&out);
        // OutputSize.x == 200 (the pane) -> R ~= 200.
        assert!((c[0] as i32 - 200).abs() <= 3, "OutputSize.x: got {c:?}");
        // FinalViewportSize.x == 200 too -> G ~= 200.
        assert!(
            (c[1] as i32 - 200).abs() <= 3,
            "FinalViewportSize.x: got {c:?}"
        );
        // FrameCount on the 6th frame was 5 -> B ~= 5.
        assert!((c[2] as i32 - 5).abs() <= 2, "FrameCount: got {c:?}");
    }

    /// `frame_count_mod = 4`: across ~10 frames the shader-visible FrameCount
    /// must cycle 0,1,2,3,0,1,… (output through the R channel).
    #[test]
    fn frame_count_mod_wraps_the_visible_counter() {
        let src = format!(
            "#version 450
layout(std140, set = 0, binding = 0) uniform UBO {{ mat4 MVP; uint FrameCount; }} global;
{VS}void main() {{ FragColor = vec4(float(global.FrameCount) / 255.0, 0.0, 0.0, 1.0); }}
"
        );
        let shader = compile_slang(&src, None).expect("compile");
        let mut pass = Pass::new(shader);
        pass.frame_count_mod = 4;

        let mut r = Renderer::new(8, 8).expect("wgpu device");
        r.set_source(&solid(8, 8, [0, 0, 0, 255]));
        r.set_chain(std::slice::from_ref(&pass)).expect("set chain");

        // Frame i is rendered with raw FrameCount = i, visible = i % 4.
        for i in 0..10u32 {
            r.render().expect("render");
            let out = r.read_back().expect("read back");
            let visible = center(&out)[0] as u32;
            assert_eq!(visible, i % 4, "frame {i}: visible FrameCount should wrap");
        }
    }

    /// A constant-color pass writing R = OutputSize.x/255 into its FBO at a known
    /// scale, so a later pass can read it back. Used to seed `Pass0`'s size.
    fn output_size_writer() -> CompiledShader {
        let src = format!(
            "#version 450
layout(std140, set = 0, binding = 0) uniform UBO {{ mat4 MVP; vec4 OutputSize; }} global;
{VS}void main() {{ FragColor = vec4(global.OutputSize.x / 255.0, global.OutputSize.y / 255.0, 0.0, 1.0); }}
"
        );
        compile_slang(&src, None).expect("compile")
    }

    /// Multi-pass: pass 1 reads `PassOutput0Size` (the spelling RetroArch
    /// canonicalizes to) and `FinalViewportSize`, proving an earlier pass's size
    /// and the pane reach a later pass. Pass 0 is an absolute-scaled intermediate
    /// so its OutputSize is a known value distinct from the pane.
    #[test]
    fn pass_output_size_and_final_viewport_reach_a_later_pass() {
        use super::{AxisScale, ScaleConfig, ScaleType};
        // Pass 1's UBO declares PassOutput0Size + FinalViewportSize (+ MVP).
        let src = format!(
            "#version 450
layout(std140, set = 0, binding = 0) uniform UBO {{
    mat4 MVP;
    vec4 PassOutput0Size;
    vec4 FinalViewportSize;
}} global;
{VS}void main() {{
    FragColor = vec4(
        global.PassOutput0Size.x / 255.0,
        global.PassOutput0Size.y / 255.0,
        global.FinalViewportSize.x / 255.0,
        1.0);
}}
"
        );
        let reader = compile_slang(&src, None).expect("compile");

        // Pass 0: absolute 120x90 intermediate; pass 1 (final) reads its size.
        let scale = ScaleConfig {
            x: AxisScale {
                ty: ScaleType::Absolute,
                factor: 120.0,
            },
            y: AxisScale {
                ty: ScaleType::Absolute,
                factor: 90.0,
            },
        };
        let pass0 = Pass::new(output_size_writer()).with_scale(scale);
        let pass1 = Pass::new(reader);
        // Pane (FinalViewportSize) = 220 wide.
        let out = render_chain(&[pass0, pass1], &solid(8, 8, [0, 0, 0, 255]), (220, 64));
        let c = center(&out);
        // PassOutput0Size = (120, 90); FinalViewportSize.x = 220.
        assert!(
            (c[0] as i32 - 120).abs() <= 3,
            "PassOutput0Size.x should be 120, got {c:?}"
        );
        assert!(
            (c[1] as i32 - 90).abs() <= 3,
            "PassOutput0Size.y should be 90, got {c:?}"
        );
        assert!(
            (c[2] as i32 - 220).abs() <= 3,
            "FinalViewportSize.x should be the pane (220), got {c:?}"
        );
    }

    /// The `PassNSize` alias (without `Output`) resolves to the same pass-output
    /// size as `PassOutputNSize` (§7/§11 — both spellings accepted).
    #[test]
    fn pass_n_size_alias_matches_pass_output_n_size() {
        use super::{AxisScale, ScaleConfig, ScaleType};
        let src = format!(
            "#version 450
layout(std140, set = 0, binding = 0) uniform UBO {{ mat4 MVP; vec4 Pass0Size; }} global;
{VS}void main() {{ FragColor = vec4(global.Pass0Size.x / 255.0, global.Pass0Size.y / 255.0, 0.0, 1.0); }}
"
        );
        let reader = compile_slang(&src, None).expect("compile");
        let scale = ScaleConfig {
            x: AxisScale {
                ty: ScaleType::Absolute,
                factor: 100.0,
            },
            y: AxisScale {
                ty: ScaleType::Absolute,
                factor: 60.0,
            },
        };
        let pass0 = Pass::new(output_size_writer()).with_scale(scale);
        let pass1 = Pass::new(reader);
        let out = render_chain(&[pass0, pass1], &solid(8, 8, [0, 0, 0, 255]), (128, 128));
        let c = center(&out);
        assert!(
            (c[0] as i32 - 100).abs() <= 3 && (c[1] as i32 - 60).abs() <= 3,
            "Pass0Size alias should equal pass 0's output (100x60), got {c:?}"
        );
    }
}

#[cfg(test)]
mod bindtable_tests {
    //! Reflection-driven texture bind-table tests (#26): `Original` vs `Source`
    //! bound to distinct textures, `PassOutputN`/`PassN` direct sampling, pass
    //! aliases (`<alias>` + `<alias>Size`), a non-default / multi-texture reflected
    //! layout, and the "K+1" sampler-attribution rule. All run on the real GPU.
    use super::{Pass, Renderer};
    use slang_compile::{compile_slang, CompiledShader};
    use source::Frame;

    /// Vertex stage + builtin UBO shared by these fixtures. The fragment body is
    /// appended per test; it declares its own texture/sampler bindings.
    const HEAD: &str = "\
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
";

    fn compile(frag: &str) -> CompiledShader {
        compile_slang(&format!("{HEAD}{frag}"), None).expect("compile #26 fixture")
    }

    /// A passthrough sampling `Source` (the standard separate tex/sampler form).
    fn passthrough() -> CompiledShader {
        compile(
            "layout(set=0, binding=1) uniform texture2D Source;
layout(set=0, binding=2) uniform sampler Smp;
void main() { FragColor = texture(sampler2D(Source, Smp), vTexCoord); }
",
        )
    }

    fn invert() -> CompiledShader {
        compile(
            "layout(set=0, binding=1) uniform texture2D Source;
layout(set=0, binding=2) uniform sampler Smp;
void main() { vec4 c = texture(sampler2D(Source, Smp), vTexCoord); FragColor = vec4(vec3(1.0)-c.rgb, c.a); }
",
        )
    }

    fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Frame {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            data.extend_from_slice(&rgba);
        }
        Frame::new(width, height, data)
    }

    fn close(a: &[u8], b: &[u8]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.abs_diff(*y) <= 3)
    }

    fn render_chain(passes: &[Pass], frame: &Frame, out: (u32, u32)) -> Frame {
        let mut r = Renderer::new(out.0, out.1).expect("wgpu device");
        r.set_source(frame);
        r.set_chain(passes).expect("set chain");
        r.render().expect("render");
        r.read_back().expect("read back")
    }

    fn center(f: &Frame) -> [u8; 4] {
        let i = (((f.height / 2) * f.width + f.width / 2) * 4) as usize;
        [f.rgba[i], f.rgba[i + 1], f.rgba[i + 2], f.rgba[i + 3]]
    }

    // ---- 1. Original and Source are bound to DISTINCT textures. ----

    /// Pass 1 inverts the source. Pass 2 samples BOTH `Source` (pass 1's inverted
    /// output) and `Original` (the unmodified source) and outputs `Original -
    /// Source`. For an input value `v`, Source = `1-v`, so the result is
    /// `v - (1-v) = 2v-1`. If Original and Source were bound to the SAME texture
    /// the result would be `0`. Proves they resolve to different textures.
    #[test]
    fn original_and_source_are_distinct_textures() {
        let combine = compile(
            "layout(set=0, binding=1) uniform texture2D Source;
layout(set=0, binding=2) uniform sampler Smp;
layout(set=0, binding=4) uniform texture2D Original;
layout(set=0, binding=5) uniform sampler OrigSmp;
void main() {
    vec3 s = texture(sampler2D(Source, Smp), vTexCoord).rgb;
    vec3 o = texture(sampler2D(Original, OrigSmp), vTexCoord).rgb;
    FragColor = vec4(o - s, 1.0);
}
",
        );
        // Source value 200/255 ≈ 0.784; expected 2*0.784-1 = 0.569 ≈ 145.
        let input = solid(4, 4, [200, 200, 200, 255]);
        let out = render_chain(&[Pass::new(invert()), Pass::new(combine)], &input, (4, 4));
        let c = center(&out);
        assert!(
            (c[0] as i32 - 145).abs() <= 6,
            "Original-Source should be 2v-1 (~145) with distinct textures, got {c:?}"
        );
        // If both were the same texture the channel would be ~0 — explicitly reject.
        assert!(
            c[0] > 60,
            "Original and Source must NOT be the same texture"
        );
    }

    // ---- 2. PassOutputN / PassN sample an earlier pass directly. ----

    /// A 3-pass chain. Pass 0 outputs RED, pass 1 outputs GREEN, pass 2 samples
    /// `PassOutput0` and must read pass 0's RED — not pass 1's GREEN (its direct
    /// Source). Spelled `PassOutput0` here and `Pass0` in the sibling test.
    fn three_pass_reads(pass_output_name: &str) -> [u8; 4] {
        let red = compile("void main() { FragColor = vec4(1.0, 0.0, 0.0, 1.0); }\n");
        let green = compile("void main() { FragColor = vec4(0.0, 1.0, 0.0, 1.0); }\n");
        let reader = compile(&format!(
            "layout(set=0, binding=1) uniform texture2D Source;
layout(set=0, binding=2) uniform sampler Smp;
layout(set=0, binding=4) uniform texture2D {name};
layout(set=0, binding=5) uniform sampler PoSmp;
void main() {{ FragColor = texture(sampler2D({name}, PoSmp), vTexCoord); }}
",
            name = pass_output_name
        ));
        let out = render_chain(
            &[Pass::new(red), Pass::new(green), Pass::new(reader)],
            &solid(4, 4, [10, 10, 10, 255]),
            (4, 4),
        );
        center(&out)
    }

    #[test]
    fn pass_output0_reads_pass0_not_the_predecessor() {
        let c = three_pass_reads("PassOutput0");
        assert!(
            close(&c, &[255, 0, 0, 255]),
            "PassOutput0 must be pass 0's RED, not pass 1's GREEN, got {c:?}"
        );
    }

    #[test]
    fn passn_alias_spelling_reads_pass0() {
        // `Pass0` is the accepted spelling for `PassOutput0` (§7/§11).
        let c = three_pass_reads("Pass0");
        assert!(
            close(&c, &[255, 0, 0, 255]),
            "Pass0 (PassOutput0 alias) must be pass 0's RED, got {c:?}"
        );
    }

    // ---- 3. Alias: <alias> and <alias>Size resolve to the aliased pass. ----

    #[test]
    fn alias_texture_and_size_resolve_to_the_aliased_pass() {
        // Pass 0 (alias FOO) outputs BLUE into a 100x60 FBO. Pass 1 is a filler.
        // Pass 2 samples `FOO` (must be pass 0's BLUE) and encodes `FOOSize`
        // (must be 100x60) into G/B... we check texture and size in two renders to
        // keep each fragment simple.
        let blue = compile("void main() { FragColor = vec4(0.0, 0.0, 1.0, 1.0); }\n");
        let filler = compile("void main() { FragColor = vec4(0.5, 0.5, 0.5, 1.0); }\n");

        // 3a. `FOO` texture resolves to pass 0's output (BLUE).
        let read_foo = compile(
            "layout(set=0, binding=1) uniform texture2D Source;
layout(set=0, binding=2) uniform sampler Smp;
layout(set=0, binding=4) uniform texture2D FOO;
layout(set=0, binding=5) uniform sampler FooSmp;
void main() { FragColor = texture(sampler2D(FOO, FooSmp), vTexCoord); }
",
        );
        let scale = super::ScaleConfig {
            x: super::AxisScale {
                ty: super::ScaleType::Absolute,
                factor: 100.0,
            },
            y: super::AxisScale {
                ty: super::ScaleType::Absolute,
                factor: 60.0,
            },
        };
        let mut p0 = Pass::new(blue.clone()).with_scale(scale);
        p0.alias = Some("FOO".to_string());
        let out = render_chain(
            &[p0, Pass::new(filler.clone()), Pass::new(read_foo)],
            &solid(8, 8, [0, 0, 0, 255]),
            (8, 8),
        );
        let c = center(&out);
        assert!(
            close(&c, &[0, 0, 255, 255]),
            "alias FOO must resolve to pass 0's BLUE output, got {c:?}"
        );

        // 3b. `FOOSize` builtin resolves to pass 0's 100x60 output size. The
        // `FOOSize` member lives in the b0 builtin UBO (alongside MVP) so the
        // reflection-driven builtin packing (#28/#26) writes it there.
        let read_foo_size = compile_slang(
            "#version 450
layout(std140, set = 0, binding = 0) uniform UBO { mat4 MVP; vec4 FOOSize; } global;
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
void main() { FragColor = vec4(global.FOOSize.x / 255.0, global.FOOSize.y / 255.0, 0.0, 1.0); }
",
            None,
        )
        .expect("compile FOOSize reader");
        let scale2 = super::ScaleConfig {
            x: super::AxisScale {
                ty: super::ScaleType::Absolute,
                factor: 100.0,
            },
            y: super::AxisScale {
                ty: super::ScaleType::Absolute,
                factor: 60.0,
            },
        };
        let mut p0b = Pass::new(blue).with_scale(scale2);
        p0b.alias = Some("FOO".to_string());
        let out2 = render_chain(
            &[p0b, Pass::new(filler), Pass::new(read_foo_size)],
            &solid(8, 8, [0, 0, 0, 255]),
            (128, 128),
        );
        let c2 = center(&out2);
        assert!(
            (c2[0] as i32 - 100).abs() <= 3 && (c2[1] as i32 - 60).abs() <= 3,
            "FOOSize must equal pass 0's output (100x60), got {c2:?}"
        );
    }

    // ---- 4. Reflection-driven layout: non-default / multiple bindings. ----

    /// A pass declaring TWO textures (`Source`@b1 and `Original`@b4) at
    /// non-adjacent bindings, with samplers at b2/b5. The reflection-driven layout
    /// must bind all of them with no validation error and read both correctly: the
    /// pass averages Source and Original (which are equal for pass 0), so the
    /// output equals the input — proving every reflected binding is wired.
    #[test]
    fn reflection_driven_layout_binds_multiple_textures_at_custom_bindings() {
        let avg = compile(
            "layout(set=0, binding=1) uniform texture2D Source;
layout(set=0, binding=2) uniform sampler Smp;
layout(set=0, binding=4) uniform texture2D Original;
layout(set=0, binding=5) uniform sampler OrigSmp;
void main() {
    vec3 s = texture(sampler2D(Source, Smp), vTexCoord).rgb;
    vec3 o = texture(sampler2D(Original, OrigSmp), vTexCoord).rgb;
    FragColor = vec4((s + o) * 0.5, 1.0);
}
",
        );
        let input = solid(4, 4, [120, 80, 40, 255]);
        // Single pass: Source == Original == input, so avg == input.
        let out = render_chain(&[Pass::new(avg)], &input, (4, 4));
        assert!(
            close(&center(&out), &[120, 80, 40, 255]),
            "both textures at custom bindings must be wired, got {:?}",
            center(&out)
        );
    }

    /// A plain passthrough (Source@b1, Smp@b2, UBO@b0) still works through the
    /// reflection-driven layout — the legacy fixed-binding case.
    #[test]
    fn legacy_single_texture_layout_still_works() {
        let input = solid(4, 4, [33, 66, 99, 255]);
        let out = render_chain(&[Pass::new(passthrough())], &input, (4, 4));
        assert!(close(&center(&out), &input.rgba[0..4]));
    }

    // ---- 5. Sampler attribution: the "K+1" rule. ----

    /// A 2x1 black/white source written into a 2x1 FBO by pass 0, then sampled at
    /// the sub-texel midpoint u=0.5 by pass 1. The sampler used for pass 1's
    /// `Source` (pass 0's output) is pass 1's own config (K=0 → K+1=1). With pass
    /// 1 `filter_linear=false` the midpoint reads a single texel (hard edge, 0 or
    /// 255); with `filter_linear=true` it blends (~127). Proves the per-bound-
    /// texture sampler is applied (the consuming pass for a direct Source).
    fn subtexel_sample(consumer_linear: bool) -> u8 {
        // Pass 0: copy the 2x1 source into a 2x1 FBO (texel0 black, texel1 white).
        let copy = compile(
            "layout(set=0, binding=1) uniform texture2D Source;
layout(set=0, binding=2) uniform sampler Smp;
void main() { FragColor = texture(sampler2D(Source, Smp), vTexCoord); }
",
        );
        let mut p0 = Pass::new(copy).with_scale(super::ScaleConfig {
            x: super::AxisScale {
                ty: super::ScaleType::Absolute,
                factor: 2.0,
            },
            y: super::AxisScale {
                ty: super::ScaleType::Absolute,
                factor: 1.0,
            },
        });
        p0.filter_linear = false; // pass 0 copies 1:1, irrelevant to the probe
                                  // Pass 1: sample pass 0's output at the exact midpoint u=0.5.
        let probe = compile(
            "layout(set=0, binding=1) uniform texture2D Source;
layout(set=0, binding=2) uniform sampler Smp;
void main() { FragColor = texture(sampler2D(Source, Smp), vec2(0.5, 0.5)); }
",
        );
        let mut p1 = Pass::new(probe);
        p1.filter_linear = consumer_linear;
        p1.wrap_mode = super::WrapMode::ClampToEdge;
        let src = Frame::new(2, 1, vec![0, 0, 0, 255, 255, 255, 255, 255]);
        let out = render_chain(&[p0, p1], &src, (8, 8));
        center(&out)[0]
    }

    #[test]
    fn sampler_attribution_uses_the_consuming_pass_for_a_direct_source() {
        // The "K+1" rule: a direct Source (pass i-1's output) is sampled with pass
        // i's config. nearest -> single texel (hard edge); linear -> the average.
        let nearest = subtexel_sample(false);
        let linear = subtexel_sample(true);
        assert!(
            nearest <= 60 || nearest >= 195,
            "nearest consumer must read a single texel (hard edge), got {nearest}"
        );
        assert!(
            (linear as i32 - 127).abs() <= 40,
            "linear consumer must blend the two texels (~127), got {linear}"
        );
        assert!(
            linear.abs_diff(nearest) > 40,
            "nearest vs linear must differ, proving the sampler is selected per texture"
        );
    }
}

#[cfg(test)]
mod feedback_tests {
    //! Feedback double-buffer tests (#24, §4): a pass reads its OWN previous-frame
    //! output via `PassFeedback0`, blending `0.5*Source + 0.5*Feedback`. Over a
    //! sequence of frames on a constant source the output converges toward the
    //! source by the recurrence `out_N = S*(1 - 0.5^(N+1))` — which is only true if
    //! feedback reads the PREVIOUS frame (a current-frame read would solve to the
    //! fixed point `S` on frame 0). All run on the real GPU and read back a pixel.
    use super::{Pass, Renderer};
    use slang_compile::{compile_slang, CompiledShader};
    use source::Frame;

    // Preamble: builtin UBO @0, Source @1 + shared sampler @2, PassFeedback0 @3.
    // Both textures share the one sampler (the renderer pairs sampler 0 with
    // texture 0 and reuses it). The fragment blends source with feedback.
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
layout(set = 0, binding = 3) uniform texture2D PassFeedback0;
";

    /// `out = 0.5*Source + 0.5*PassFeedback0` — the classic decay/accumulate
    /// feedback pass reading its own previous-frame output.
    fn half_blend_feedback() -> CompiledShader {
        compile_slang(
            &format!(
                "{PREAMBLE}void main() {{ \
                 vec4 s = texture(sampler2D(Source, Smp), vTexCoord); \
                 vec4 f = texture(sampler2D(PassFeedback0, Smp), vTexCoord); \
                 FragColor = 0.5 * s + 0.5 * f; }}\n"
            ),
            None,
        )
        .expect("compile feedback fixture")
    }

    /// A plain passthrough (Source only) used as the final pass when the feedback
    /// pass is an INTERMEDIATE pass.
    fn passthrough() -> CompiledShader {
        compile_slang(
            "\
#version 450
layout(std140, set = 0, binding = 0) uniform UBO { mat4 MVP; vec4 SourceSize; vec4 OriginalSize; vec4 OutputSize; uint FrameCount; } global;
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
",
            None,
        )
        .expect("compile passthrough")
    }

    fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Frame {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            data.extend_from_slice(&rgba);
        }
        Frame::new(width, height, data)
    }

    fn center_r(f: &Frame) -> u8 {
        let i = (((f.height / 2) * f.width + f.width / 2) * 4) as usize;
        f.rgba[i]
    }

    #[test]
    fn single_pass_self_feedback_converges_toward_source() {
        // A single feedback pass (also the FINAL pass — exercises the no-FBO →
        // owns-FBO + blit upgrade). Source R = 200. The R channel must follow
        // out_N = 200*(1 - 0.5^(N+1)): 100, 150, 175, 187.5, ...
        let src = solid(4, 4, [200, 0, 0, 255]);
        let mut r = Renderer::new(4, 4).expect("wgpu device");
        r.set_source(&src);
        r.set_chain(&[Pass::new(half_blend_feedback())])
            .expect("set chain");

        let mut seq = Vec::new();
        for _ in 0..4 {
            r.render().expect("render");
            seq.push(center_r(&r.read_back().expect("read back")));
        }
        // Frame 0 ~100 (NOT 200): proves feedback read the PREVIOUS (cold/black)
        // frame, not the current output (which would solve to the fixed point 200).
        assert!(
            (seq[0] as i32 - 100).abs() <= 6,
            "frame 0 should be ~100 (0.5*200 + 0.5*0), got {}; a value near 200 \
             would mean the shader read the CURRENT frame's output",
            seq[0]
        );
        // Strictly increasing toward 200 (monotonic accumulation).
        assert!(
            seq[0] < seq[1] && seq[1] < seq[2] && seq[2] < seq[3],
            "feedback must accumulate monotonically toward the source, got {seq:?}"
        );
        // Approaches the expected analytic values.
        for (n, &got) in seq.iter().enumerate() {
            let want = 200.0 * (1.0 - 0.5_f32.powi(n as i32 + 1));
            assert!(
                (got as f32 - want).abs() <= 6.0,
                "frame {n}: want ~{want:.1}, got {got} (seq {seq:?})"
            );
        }
    }

    #[test]
    fn intermediate_feedback_pass_reads_previous_frame() {
        // The feedback pass is pass 0 (an INTERMEDIATE pass; owns an FBO normally),
        // followed by a passthrough final pass. Same convergence proves the
        // previous-frame read on the non-upgrade path.
        let src = solid(4, 4, [200, 0, 0, 255]);
        let mut r = Renderer::new(4, 4).expect("wgpu device");
        r.set_source(&src);
        r.set_chain(&[Pass::new(half_blend_feedback()), Pass::new(passthrough())])
            .expect("set chain");

        let mut seq = Vec::new();
        for _ in 0..3 {
            r.render().expect("render");
            seq.push(center_r(&r.read_back().expect("read back")));
        }
        assert!(
            (seq[0] as i32 - 100).abs() <= 6,
            "frame 0 should be ~100 (previous frame was cold black), got {}",
            seq[0]
        );
        assert!(
            seq[0] < seq[1] && seq[1] < seq[2],
            "feedback through an intermediate pass must still accumulate, got {seq:?}"
        );
    }

    #[test]
    fn rebuilding_the_chain_resets_feedback() {
        // After accumulating several frames, re-setting the chain must reset the
        // feedback buffers to cold black, so the next frame 0 is ~100 again.
        let src = solid(4, 4, [200, 0, 0, 255]);
        let mut r = Renderer::new(4, 4).expect("wgpu device");
        r.set_source(&src);
        r.set_chain(&[Pass::new(half_blend_feedback())])
            .expect("set chain");
        for _ in 0..5 {
            r.render().expect("render");
            let _ = r.read_back().expect("read back");
        }
        // Rebuild: fresh PassResources → feedback FBOs reallocate + clear.
        r.set_chain(&[Pass::new(half_blend_feedback())])
            .expect("rebuild chain");
        r.render().expect("render");
        let after = center_r(&r.read_back().expect("read back"));
        assert!(
            (after as i32 - 100).abs() <= 6,
            "rebuild should reset feedback to cold black (frame 0 ~100), got {after}"
        );
    }

    #[test]
    fn non_feedback_chain_is_unaffected() {
        // A chain with no feedback references renders deterministically the same on
        // every frame (no double-buffer, no swap-induced drift).
        let src = solid(4, 4, [123, 0, 0, 255]);
        let mut r = Renderer::new(4, 4).expect("wgpu device");
        r.set_source(&src);
        r.set_chain(&[Pass::new(passthrough())]).expect("set chain");
        r.render().expect("render");
        let a = center_r(&r.read_back().expect("read back"));
        r.render().expect("render");
        let b = center_r(&r.read_back().expect("read back"));
        assert_eq!(a, b, "a non-feedback pass must be frame-stable");
        assert!(
            (a as i32 - 123).abs() <= 2,
            "passthrough should reproduce source"
        );
    }
}

#[cfg(test)]
mod feedback_size_tests {
    //! `PassFeedbackKSize` / `<alias>FeedbackSize` builtin semantics (#24, §4).
    use crate::uniforms::{size_vec, BuiltinValues};

    #[test]
    fn pass_feedback_size_resolves_for_any_index() {
        let v = BuiltinValues {
            pass_feedback_sizes: vec![size_vec(320, 240), size_vec(64, 64)],
            ..Default::default()
        };
        // Both indices resolve (feedback is time-causal, so no "earlier-only" rule).
        assert_eq!(
            v.member_bytes("PassFeedback0Size"),
            Some(size_vec_bytes(320, 240))
        );
        assert_eq!(
            v.member_bytes("PassFeedback1Size"),
            Some(size_vec_bytes(64, 64))
        );
        // Out of range → None (member stays zero).
        assert_eq!(v.member_bytes("PassFeedback9Size"), None);
        // The plain output-size spelling is unaffected and distinct.
        assert_eq!(v.member_bytes("PassFeedbackXSize"), None);
    }

    #[test]
    fn alias_feedback_size_resolves_by_name() {
        let mut v = BuiltinValues::default();
        v.alias_feedback_sizes
            .insert("PREV".to_string(), size_vec(128, 96));
        v.alias_sizes.insert("PREV".to_string(), size_vec(128, 96));
        // (Two inserts on the same value → keep the readable mutate-after-default form.)
        assert_eq!(
            v.member_bytes("PREVFeedbackSize"),
            Some(size_vec_bytes(128, 96))
        );
        // The non-feedback alias size still resolves.
        assert_eq!(v.member_bytes("PREVSize"), Some(size_vec_bytes(128, 96)));
    }

    fn size_vec_bytes(w: u32, h: u32) -> Vec<u8> {
        size_vec(w, h)
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect()
    }
}

#[cfg(test)]
mod history_tests {
    //! Frame history ring tests (#25, §5): `OriginalHistoryK` samples the source
    //! frame K frames ago. A numbered sequence proves `History1/2/3` hold frames
    //! N-1/N-2/N-3; warm-up reads cold black; reload / rebuild reset the ring.
    use super::{Pass, Renderer};
    use slang_compile::{compile_slang, CompiledShader};
    use source::Frame;

    // A single pass sampling OriginalHistory1/2/3 into output R/G/B, so one
    // read-back decodes which past frame each slot holds. One shared sampler.
    const SHADER: &str = "\
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
layout(set = 0, binding = 1) uniform texture2D OriginalHistory1;
layout(set = 0, binding = 2) uniform sampler Smp;
layout(set = 0, binding = 3) uniform texture2D OriginalHistory2;
layout(set = 0, binding = 4) uniform texture2D OriginalHistory3;
void main() {
    FragColor = vec4(
        texture(sampler2D(OriginalHistory1, Smp), vTexCoord).r,
        texture(sampler2D(OriginalHistory2, Smp), vTexCoord).r,
        texture(sampler2D(OriginalHistory3, Smp), vTexCoord).r,
        1.0);
}
";

    fn history_shader() -> CompiledShader {
        compile_slang(SHADER, None).expect("compile history fixture")
    }

    fn solid(width: u32, height: u32, r: u8) -> Frame {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            data.extend_from_slice(&[r, 0, 0, 255]);
        }
        Frame::new(width, height, data)
    }

    fn center(f: &Frame) -> [u8; 3] {
        let i = (((f.height / 2) * f.width + f.width / 2) * 4) as usize;
        [f.rgba[i], f.rgba[i + 1], f.rgba[i + 2]]
    }

    fn close(got: [u8; 3], want: [u8; 3]) -> bool {
        got.iter()
            .zip(want)
            .all(|(g, w)| (*g as i32 - w as i32).abs() <= 3)
    }

    #[test]
    fn history_1_2_3_sample_frames_n_minus_1_2_3() {
        // Frames f0..f3 with R = 200,150,100,50 (oldest..newest). After advancing
        // through all four (current Original = f3), the ring holds f2,f1,f0, so
        // History1=f2(100), History2=f1(150), History3=f0(200).
        let mut r = Renderer::new(4, 4).expect("wgpu device");
        r.set_source(&solid(4, 4, 200)); // f0
        r.set_chain(&[Pass::new(history_shader())])
            .expect("set chain");
        r.advance_source(&solid(4, 4, 150)); // f1: ring {f0}
        r.advance_source(&solid(4, 4, 100)); // f2: ring {f1,f0}
        r.advance_source(&solid(4, 4, 50)); // f3: ring {f2,f1,f0}
        r.render().expect("render");
        let got = center(&r.read_back().expect("read back"));
        assert!(
            close(got, [100, 150, 200]),
            "History1/2/3 must hold f2/f1/f0 (R 100/150/200), got {got:?}"
        );
    }

    #[test]
    fn warmup_reads_cold_black_until_the_ring_fills() {
        let mut r = Renderer::new(4, 4).expect("wgpu device");
        r.set_source(&solid(4, 4, 200)); // f0
        r.set_chain(&[Pass::new(history_shader())])
            .expect("set chain");

        // Frame 0: ring empty → all three history slots cold black.
        r.render().expect("render");
        assert!(
            close(center(&r.read_back().expect("read back")), [0, 0, 0]),
            "cold start: History1/2/3 must all read black"
        );

        // After one advance: History1 = f0 (200), History2/3 still cold (0).
        r.advance_source(&solid(4, 4, 150)); // f1: ring {f0}
        r.render().expect("render");
        let got = center(&r.read_back().expect("read back"));
        assert!(
            close(got, [200, 0, 0]),
            "warm-up: only History1 (=f0, R200) populated, rest cold; got {got:?}"
        );
    }

    #[test]
    fn reload_and_rebuild_reset_the_ring() {
        let mut r = Renderer::new(4, 4).expect("wgpu device");
        r.set_source(&solid(4, 4, 200));
        r.set_chain(&[Pass::new(history_shader())])
            .expect("set chain");
        for v in [150u8, 100, 50] {
            r.advance_source(&solid(4, 4, v));
        }
        r.render().expect("render");
        assert!(
            !close(center(&r.read_back().expect("read")), [0, 0, 0]),
            "ring filled"
        );

        // Reload (set_source) resets history → cold black again.
        r.set_source(&solid(4, 4, 200));
        r.render().expect("render");
        assert!(
            close(center(&r.read_back().expect("read")), [0, 0, 0]),
            "set_source (reload/seek) must reset the history ring"
        );

        // Refill, then a pipeline rebuild (set_chain) must also reset it.
        for v in [150u8, 100, 50] {
            r.advance_source(&solid(4, 4, v));
        }
        r.set_chain(&[Pass::new(history_shader())])
            .expect("rebuild");
        r.render().expect("render");
        assert!(
            close(center(&r.read_back().expect("read")), [0, 0, 0]),
            "set_chain (pipeline rebuild) must reset the history ring"
        );
    }

    #[test]
    fn history_does_not_advance_on_repeated_render() {
        // Rendering the same source many times must NOT advance history (it is per
        // SOURCE frame, not per render): History1 stays cold until advance_source.
        let mut r = Renderer::new(4, 4).expect("wgpu device");
        r.set_source(&solid(4, 4, 200));
        r.set_chain(&[Pass::new(history_shader())])
            .expect("set chain");
        for _ in 0..5 {
            r.render().expect("render");
        }
        assert!(
            close(center(&r.read_back().expect("read")), [0, 0, 0]),
            "repeated render must not fill history (advance is per source frame)"
        );
    }
}

#[cfg(test)]
mod history_size_tests {
    //! `OriginalHistoryKSize` builtin semantics (#25, §5).
    use crate::uniforms::{size_vec, BuiltinValues};

    fn bytes(w: u32, h: u32) -> Vec<u8> {
        size_vec(w, h)
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect()
    }

    #[test]
    fn history_size_resolves_per_slot_and_zero_is_original() {
        let v = BuiltinValues {
            original_size: size_vec(320, 240),
            original_history_sizes: vec![size_vec(64, 48), size_vec(32, 24)],
            ..Default::default()
        };
        // History0Size ≡ OriginalSize.
        assert_eq!(
            v.member_bytes("OriginalHistory0Size"),
            Some(bytes(320, 240))
        );
        // History1/2 → ring slots 0/1.
        assert_eq!(v.member_bytes("OriginalHistory1Size"), Some(bytes(64, 48)));
        assert_eq!(v.member_bytes("OriginalHistory2Size"), Some(bytes(32, 24)));
        // Past the populated depth → None (member stays zero).
        assert_eq!(v.member_bytes("OriginalHistory5Size"), None);
    }
}

#[cfg(test)]
mod lut_tests {
    //! LUT texture tests (#27, §7): a `textures=` entry binds by name (`<NAME>`)
    //! with its OWN sampler (per-LUT filter/wrap), independent of pass samplers.
    use super::{LutSpec, Pass, Renderer, WrapMode};
    use slang_compile::{compile_slang, CompiledShader};
    use source::Frame;

    // A pass that samples LUT "PAL" — a texture named neither Source/Original/Pass*
    // /History*, so it classifies as a LUT (#27) and binds the registered LUT.
    fn lut_at(coord: &str) -> CompiledShader {
        compile_slang(
            &format!(
                "\
#version 450
layout(std140, set = 0, binding = 0) uniform UBO {{ mat4 MVP; }} global;
#pragma stage vertex
layout(location = 0) in vec4 Position;
layout(location = 1) in vec2 TexCoord;
layout(location = 0) out vec2 vTexCoord;
void main() {{ gl_Position = global.MVP * Position; vTexCoord = TexCoord; }}
#pragma stage fragment
layout(location = 0) in vec2 vTexCoord;
layout(location = 0) out vec4 FragColor;
layout(set = 0, binding = 1) uniform texture2D PAL;
layout(set = 0, binding = 2) uniform sampler Smp;
void main() {{ FragColor = texture(sampler2D(PAL, Smp), {coord}); }}
"
            ),
            None,
        )
        .expect("compile LUT fixture")
    }

    /// A 2×1 LUT: left texel red, right texel green.
    fn red_green_lut() -> Frame {
        Frame::new(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 255])
    }

    fn black_source() -> Frame {
        Frame::new(4, 4, vec![0; 4 * 4 * 4])
    }

    fn spec(name: &str, image: Frame, filter_linear: bool) -> LutSpec {
        LutSpec {
            name: name.to_string(),
            image,
            filter_linear,
            wrap_mode: WrapMode::ClampToEdge,
            mipmap: false,
        }
    }

    fn px(f: &Frame, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * f.width + x) * 4) as usize;
        [f.rgba[i], f.rgba[i + 1], f.rgba[i + 2], f.rgba[i + 3]]
    }

    #[test]
    fn lut_binds_by_name_and_samples_its_content() {
        // Sample PAL across the output's uv.x with NEAREST: left half = red texel,
        // right half = green texel. Proves the LUT binds by name with real content.
        let mut r = Renderer::new(8, 8).expect("wgpu device");
        r.set_source(&black_source());
        r.set_luts(vec![spec("PAL", red_green_lut(), false)]);
        r.set_chain(&[Pass::new(lut_at("vTexCoord"))])
            .expect("chain");
        r.render().expect("render");
        let out = r.read_back().expect("read back");
        let left = px(&out, 1, 4);
        let right = px(&out, 6, 4);
        assert!(
            left[0] > 200 && left[1] < 40,
            "left should sample the red LUT texel, got {left:?}"
        );
        assert!(
            right[1] > 200 && right[0] < 40,
            "right should sample the green LUT texel, got {right:?}"
        );
    }

    #[test]
    fn per_lut_filter_nearest_vs_linear_differ_at_the_texel_boundary() {
        // Sample at the exact 2×1 boundary uv=(0.5,0.5). NEAREST picks one texel
        // (one channel ~0); LINEAR blends both (~128,128). Proves the LUT's OWN
        // filter setting is applied.
        let render = |linear: bool| {
            let mut r = Renderer::new(4, 4).expect("wgpu device");
            r.set_source(&black_source());
            r.set_luts(vec![spec("PAL", red_green_lut(), linear)]);
            r.set_chain(&[Pass::new(lut_at("vec2(0.5, 0.5)"))])
                .expect("chain");
            r.render().expect("render");
            px(&r.read_back().expect("read"), 2, 2)
        };
        let nearest = render(false);
        let linear = render(true);
        assert!(
            (nearest[0] < 40 && nearest[1] > 200) || (nearest[0] > 200 && nearest[1] < 40),
            "nearest must pick a single texel (one channel ~0), got {nearest:?}"
        );
        assert!(
            (linear[0] as i32 - 128).abs() <= 50 && (linear[1] as i32 - 128).abs() <= 50,
            "linear must blend the two texels (~128,128), got {linear:?}"
        );
    }

    #[test]
    fn unregistered_lut_falls_back_to_black() {
        // No LUT registered → the name binds the 1×1 black placeholder, not garbage.
        let mut r = Renderer::new(4, 4).expect("wgpu device");
        r.set_source(&black_source());
        r.set_luts(vec![]); // none registered
        r.set_chain(&[Pass::new(lut_at("vTexCoord"))])
            .expect("chain");
        r.render().expect("render");
        let p = px(&r.read_back().expect("read"), 2, 2);
        assert_eq!(
            [p[0], p[1], p[2]],
            [0, 0, 0],
            "unregistered LUT → black, got {p:?}"
        );
    }

    #[test]
    fn set_luts_replaces_the_previous_set() {
        // Re-registering replaces the LUT: a second set_luts wins.
        let mut r = Renderer::new(4, 4).expect("wgpu device");
        r.set_source(&black_source());
        r.set_luts(vec![spec("PAL", red_green_lut(), false)]);
        r.set_chain(&[Pass::new(lut_at("vec2(0.25,0.5)"))])
            .expect("chain"); // left=red
        r.render().expect("render");
        assert!(
            px(&r.read_back().expect("read"), 2, 2)[0] > 200,
            "first LUT red"
        );

        // Replace PAL with a solid-blue 1×1 LUT.
        r.set_luts(vec![spec(
            "PAL",
            Frame::new(1, 1, vec![0, 0, 255, 255]),
            false,
        )]);
        r.render().expect("render");
        let p = px(&r.read_back().expect("read"), 2, 2);
        assert!(
            p[2] > 200 && p[0] < 40,
            "replaced LUT should be blue, got {p:?}"
        );
    }
}

#[cfg(test)]
mod lut_size_tests {
    //! `<NAME>Size` builtin for a registered LUT (#27, §7).
    use crate::uniforms::{size_vec, BuiltinValues};

    #[test]
    fn lut_name_size_resolves() {
        let mut v = BuiltinValues::default();
        v.lut_sizes.insert("PAL".to_string(), size_vec(16, 16));
        let want: Vec<u8> = size_vec(16, 16)
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        assert_eq!(v.member_bytes("PALSize"), Some(want));
        // An unregistered LUT size is unknown (member stays zero).
        assert_eq!(v.member_bytes("OtherSize"), None);
    }
}

#[cfg(test)]
mod viewport_tests {
    //! Simulated-viewport tests (#30, Architecture §D, §9): the final pass renders
    //! at the simulated viewport (output) resolution — NOT the pane — and is then
    //! composited into the pane with black letterbox/pillarbox bars. Real GPU,
    //! read-back style (mirrors `chain_tests`): a probe shader encodes a `*Size`
    //! builtin into a uniform color so the read-back decodes the size the final
    //! pass actually saw, and a solid-color content shader lets us read the bars.
    use super::{Pass, Renderer, ViewportConfig};
    use slang_compile::{compile_slang, CompiledShader};
    use source::Frame;

    // Builtin UBO carrying OutputSize + FinalViewportSize + separate tex/sampler.
    const PREAMBLE: &str = "\
#version 450
layout(std140, set = 0, binding = 0) uniform UBO {
    mat4 MVP;
    vec4 SourceSize;
    vec4 OriginalSize;
    vec4 OutputSize;
    vec4 FinalViewportSize;
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
        compile_slang(&format!("{PREAMBLE}{frag_body}"), None).expect("compile #30 fixture")
    }

    /// Encodes the final pass's own `OutputSize` into a uniform color (R = w/512,
    /// G = h/512) so the read-back decodes the size the final pass rendered at.
    fn output_size_probe() -> CompiledShader {
        compile("void main() { FragColor = vec4(global.OutputSize.x / 512.0, global.OutputSize.y / 512.0, 0.0, 1.0); }\n")
    }

    /// Encodes `FinalViewportSize` the same way.
    fn final_viewport_size_probe() -> CompiledShader {
        compile("void main() { FragColor = vec4(global.FinalViewportSize.x / 512.0, global.FinalViewportSize.y / 512.0, 0.0, 1.0); }\n")
    }

    /// A pass that ignores its input and writes a constant color — used to fill the
    /// simulated-viewport content region so the surrounding letterbox bars read as
    /// the (cleared) black and the content reads the constant.
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

    fn size_close(got: (u32, u32), want: (u32, u32)) -> bool {
        got.0.abs_diff(want.0) <= 3 && got.1.abs_diff(want.1) <= 3
    }

    /// Render `passes` with a simulated viewport set, returning the pane read-back.
    fn render_with_viewport(
        passes: &[Pass],
        frame: &Frame,
        pane: (u32, u32),
        sim: ViewportConfig,
    ) -> Frame {
        let mut r = Renderer::new(pane.0, pane.1).expect("wgpu device");
        r.set_source(frame);
        r.set_simulated_viewport(Some(sim));
        r.set_chain(passes).expect("set chain");
        r.render().expect("render");
        r.read_back().expect("read back")
    }

    fn decode(b: u8) -> u32 {
        (b as f32 / 255.0 * 512.0).round() as u32
    }

    fn px(f: &Frame, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * f.width + x) * 4) as usize;
        [f.rgba[i], f.rgba[i + 1], f.rgba[i + 2], f.rgba[i + 3]]
    }

    #[test]
    fn final_pass_output_size_is_the_simulated_viewport_not_the_pane() {
        // Pane 256x256, simulated viewport 100x80 with a source of the SAME aspect
        // (50x40 -> aspect-fit fills 100x80 exactly, content == output, no bars).
        // The final OutputSize probe must decode the SIMULATED content size
        // (100x80), NOT the pane (256). A center pixel is inside the (full-pane)
        // content, so it reads the encoded size cleanly.
        let sim = ViewportConfig {
            width: 100,
            height: 80,
            integer_scale: false,
        };
        let out = render_with_viewport(
            &[Pass::new(output_size_probe())],
            &solid(50, 40, [0, 0, 0, 255]),
            (256, 256),
            sim,
        );
        assert_eq!(
            (out.width, out.height),
            (256, 256),
            "read-back is pane-sized"
        );
        let c = px(&out, out.width / 2, out.height / 2);
        let got = (decode(c[0]), decode(c[1]));
        assert!(
            size_close(got, (100, 80)),
            "final pass must see OutputSize == the simulated content (100x80), not \
             the pane (256); got {got:?}"
        );
    }

    #[test]
    fn final_viewport_size_is_the_simulated_viewport_not_the_pane() {
        // Same setup, probing FinalViewportSize instead of OutputSize.
        let sim = ViewportConfig {
            width: 120,
            height: 90,
            integer_scale: false,
        };
        let out = render_with_viewport(
            &[Pass::new(final_viewport_size_probe())],
            &solid(40, 30, [0, 0, 0, 255]),
            (300, 300),
            sim,
        );
        let c = px(&out, out.width / 2, out.height / 2);
        let got = (decode(c[0]), decode(c[1]));
        assert!(
            size_close(got, (120, 90)),
            "FinalViewportSize must be the simulated viewport (120x90), not the pane \
             (300); got {got:?}"
        );
    }

    #[test]
    fn viewport_scaled_intermediate_uses_the_simulated_viewport() {
        // A `viewport × 1.0` INTERMEDIATE pass must size to the simulated content,
        // not the pane. Pane 256x256; simulated 100x80 (source 50x40 -> content
        // 100x80). Pass 0 (viewport×1, the OutputSize probe) thus has a 100x80 FBO;
        // a passthrough final pass carries the (uniform) encoded size to the pane.
        use super::{AxisScale, ScaleConfig};
        let sim = ViewportConfig {
            width: 100,
            height: 80,
            integer_scale: false,
        };
        let pass0 = Pass::new(output_size_probe()).with_scale(ScaleConfig {
            x: AxisScale::VIEWPORT_1X,
            y: AxisScale::VIEWPORT_1X,
        });
        let passthrough =
            compile("void main() { FragColor = texture(sampler2D(Source, Smp), vTexCoord); }\n");
        let out = render_with_viewport(
            &[pass0, Pass::new(passthrough)],
            &solid(50, 40, [0, 0, 0, 255]),
            (256, 256),
            sim,
        );
        let c = px(&out, out.width / 2, out.height / 2);
        let got = (decode(c[0]), decode(c[1]));
        assert!(
            size_close(got, (100, 80)),
            "viewport×1 intermediate must size to the simulated viewport (100x80), \
             not the pane (256); got {got:?}"
        );
    }

    #[test]
    fn integer_scale_letterboxes_the_content_in_the_pane() {
        // Pane == output (100x100) so the composite maps the content rect 1:1 into
        // the pane. Integer-scale a 30x30 source: n = floor(100/30) = 3 -> content
        // 90x90 centered at offset (5,5). The final pass writes a constant RED into
        // the content FBO; the composite clears the pane to black and draws the
        // content into the centered 90x90 sub-rect — so:
        //   - the pane CENTER (50,50) reads red (inside the content), and
        //   - the pane CORNER (1,1) reads black (a letterbox bar; offset is 5px).
        let sim = ViewportConfig {
            width: 100,
            height: 100,
            integer_scale: true,
        };
        let out = render_with_viewport(
            &[Pass::new(constant([1.0, 0.0, 0.0, 1.0]))],
            &solid(30, 30, [0, 0, 0, 255]),
            (100, 100),
            sim,
        );
        assert_eq!((out.width, out.height), (100, 100));
        let center = px(&out, 50, 50);
        assert!(
            center[0] > 200 && center[1] < 60 && center[2] < 60,
            "content center should be red, got {center:?}"
        );
        // Corner is well inside the 5px bar (sample (1,1) to dodge edge sampling).
        let corner = px(&out, 1, 1);
        assert!(
            corner[0] < 40 && corner[1] < 40 && corner[2] < 40,
            "letterbox corner should be black, got {corner:?}"
        );
        // And a point just inside the bar on the left edge mid-height is black,
        // while just inside the content is red — proving the bar is at the offset.
        assert!(px(&out, 2, 50)[0] < 40, "left bar at x=2 should be black");
        assert!(px(&out, 50, 50)[0] > 200, "content at x=50 should be red");
    }

    #[test]
    fn clearing_the_simulated_viewport_reverts_to_full_pane() {
        // Setting then clearing the simulated viewport must restore the pre-#30
        // direct path: the no-scale final pass fills the whole pane (no bars).
        let mut r = Renderer::new(64, 64).expect("wgpu device");
        r.set_source(&solid(30, 30, [0, 0, 0, 255]));
        r.set_chain(&[Pass::new(constant([1.0, 0.0, 0.0, 1.0]))])
            .expect("set chain");

        // With an integer-scale simulated viewport the corner is a black bar.
        r.set_simulated_viewport(Some(ViewportConfig {
            width: 64,
            height: 64,
            integer_scale: true,
        }));
        r.render().expect("render");
        let with_sim = r.read_back().expect("read back");
        assert!(
            px(&with_sim, 1, 1)[0] < 40,
            "simulated viewport: corner is a letterbox bar"
        );

        // Clear it: the final pass again fills the whole pane, so the corner is red.
        r.set_simulated_viewport(None);
        r.render().expect("render");
        let no_sim = r.read_back().expect("read back");
        assert!(
            px(&no_sim, 1, 1)[0] > 200,
            "after clearing, the whole pane is content (no bars), got {:?}",
            px(&no_sim, 1, 1)
        );
    }
}
