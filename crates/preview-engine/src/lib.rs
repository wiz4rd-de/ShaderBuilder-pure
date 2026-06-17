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

pub use bindtable::{PlaceholderResolver, TextureClass, TextureResolver};
pub use frame::{FrameHeader, FRAME_HEADER_LEN, FRAME_MAGIC, FRAME_VERSION, PIXEL_FORMAT_RGBA8};
pub use pass::{AxisScale, Pass, ScaleConfig, ScaleType, WrapMode};
pub use render_source::{RenderCommand, RenderSource, DEFAULT_SHADER};
pub use renderer::{Renderer, RendererError, OFFSCREEN_FORMAT};
pub use uniforms::{BuiltinUniforms, BuiltinValues, ParamDef, ParamStore, ParamView};

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
