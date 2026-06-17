//! GPU end-to-end proof that **combined `sampler2D`** slang shaders — the form
//! every real RetroArch `.slang` uses — now build a wgpu pipeline and render
//! correctly, after `slang_compile`'s SPIR-V sampler-splitting transform.
//!
//! ## What this guards
//!
//! glslang compiles `uniform sampler2D Source; … texture(Source, uv)` to SPIR-V
//! with a *combined* `OpTypeSampledImage`. WebGPU has no combined samplers, so
//! both `slang_compile::reflect` (naga) and wgpu's pipeline ingestion (naga) used
//! to reject it (`invalid id %14`). `slang_compile` now rewrites that SPIR-V into
//! separate image + sampler bindings before anyone sees it. These tests prove the
//! whole chain works *through the GPU*: the split SPIR-V (a) builds a pipeline and
//! (b) binds its image and split-out sampler consistently — verified by the output
//! reproducing a solid-color source.
//!
//! ## Why solid-color sources + region means
//!
//! A passthrough over a *solid* fill reproduces that fill regardless of any
//! orientation/filtering subtlety, so the assertion is robust on both hardware and
//! the lavapipe software adapter CI uses. We compare region means with a generous
//! tolerance (never exact pixels) for the same reason the curvature e2e does.
//!
//! Run with `WGPU_BACKEND=vulkan … -- --test-threads=1` (concurrent wgpu device
//! creation SIGSEGVs on multi-GPU boxes).

use preview_engine::Renderer;
use source::Frame;

const OUT_W: u32 = 64;
const OUT_H: u32 = 64;
const SRC_W: u32 = 64;
const SRC_H: u32 = 64;

/// A RetroArch-faithful passthrough declaring its input the **combined** way
/// (`uniform sampler2D Source`) and sampling with `texture()`. This is the exact
/// shape that used to fail with `invalid id %14`.
const COMBINED_PASSTHROUGH: &str = "\
#version 450
layout(set = 0, binding = 0) uniform UBO { mat4 MVP; } global;
#pragma stage vertex
layout(location = 0) in vec4 Position;
layout(location = 1) in vec2 TexCoord;
layout(location = 0) out vec2 vTexCoord;
void main() { gl_Position = global.MVP * Position; vTexCoord = TexCoord; }
#pragma stage fragment
layout(location = 0) in vec2 vTexCoord;
layout(location = 0) out vec4 FragColor;
layout(set = 0, binding = 1) uniform sampler2D Source;
void main() { FragColor = texture(Source, vTexCoord); }
";

/// The hand-written **separate**-sampler passthrough (`texture2D` + `sampler`) the
/// engine was originally built around — the control proving the combined form
/// renders the *same* result, and that the split is a no-op on the separate form.
const SEPARATE_PASSTHROUGH: &str = "\
#version 450
layout(set = 0, binding = 0) uniform UBO { mat4 MVP; } global;
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

/// A combined-sampler shader that uses `texelFetch` (+`textureSize`) — the path
/// that lowers the combined sampler to a plain image via `OpImage`/`OpImageFetch`/
/// `OpImageQuerySizeLod`, exercising the image-domain branch of the transform. It
/// reads the exact source texel for the fragment, so over a solid fill it also
/// reproduces the source.
const COMBINED_TEXEL_FETCH: &str = "\
#version 450
layout(std140, set = 0, binding = 0) uniform UBO { mat4 MVP; vec4 SourceSize; } global;
#pragma stage vertex
layout(location = 0) in vec4 Position;
layout(location = 1) in vec2 TexCoord;
layout(location = 0) out vec2 vTexCoord;
void main() { gl_Position = global.MVP * Position; vTexCoord = TexCoord; }
#pragma stage fragment
layout(location = 0) in vec2 vTexCoord;
layout(location = 0) out vec4 FragColor;
layout(set = 0, binding = 1) uniform sampler2D Source;
void main() {
    ivec2 sz = textureSize(Source, 0);
    ivec2 c = clamp(ivec2(vTexCoord * vec2(sz)), ivec2(0), sz - ivec2(1));
    FragColor = texelFetch(Source, c, 0);
}
";

/// A `SRC_W x SRC_H` frame filled with one solid opaque color.
fn solid(color: [u8; 3]) -> Frame {
    let mut rgba = Vec::with_capacity((SRC_W * SRC_H * 4) as usize);
    for _ in 0..(SRC_W * SRC_H) {
        rgba.extend_from_slice(&[color[0], color[1], color[2], 255]);
    }
    Frame::new(SRC_W, SRC_H, rgba)
}

/// Compile slang source and render it over `src` into an `OUT_W x OUT_H` pane.
fn render(slang: &str, src: &Frame) -> Frame {
    let shader = slang_compile::compile_slang(slang, None).expect("compile slang");
    let mut r = Renderer::new(OUT_W, OUT_H).expect("wgpu device (set WGPU_BACKEND/VK_ICD on CI)");
    r.set_source(src);
    r.set_shader(&shader);
    r.render().expect("render");
    r.read_back().expect("read back")
}

/// Mean RGB over the centre region of a frame (avoids any edge filtering).
fn centre_mean(f: &Frame) -> [f64; 3] {
    let (x0, y0, x1, y1) = (OUT_W / 4, OUT_H / 4, OUT_W * 3 / 4, OUT_H * 3 / 4);
    let mut acc = [0f64; 3];
    let mut n = 0f64;
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * f.width + x) * 4) as usize;
            acc[0] += f.rgba[i] as f64;
            acc[1] += f.rgba[i + 1] as f64;
            acc[2] += f.rgba[i + 2] as f64;
            n += 1.0;
        }
    }
    [acc[0] / n, acc[1] / n, acc[2] / n]
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Test 2: a combined-`sampler2D` passthrough builds a pipeline and reproduces a
/// solid source — the headline proof that the split SPIR-V renders correctly and
/// its image + split-out sampler bind consistently.
#[test]
fn combined_sampler_passthrough_reproduces_solid_source() {
    let color = [37u8, 150, 190];
    let out = render(COMBINED_PASSTHROUGH, &solid(color));
    let got = centre_mean(&out);
    let want = [color[0] as f64, color[1] as f64, color[2] as f64];
    assert!(
        dist(got, want) < 8.0,
        "combined-sampler passthrough should reproduce the source color {want:?}, got {got:?}"
    );
}

/// Test 2 (control): the combined and the hand-written separate passthrough render
/// the *same* solid source to the same color — combined now behaves like separate.
#[test]
fn combined_matches_separate_sampler_control() {
    let color = [90u8, 40, 160];
    let combined = centre_mean(&render(COMBINED_PASSTHROUGH, &solid(color)));
    let separate = centre_mean(&render(SEPARATE_PASSTHROUGH, &solid(color)));
    assert!(
        dist(combined, separate) < 6.0,
        "combined {combined:?} and separate {separate:?} passthrough must match"
    );
}

/// Test 3: a `texelFetch` combined-sampler shader (the `OpImage`/`OpImageFetch`
/// path) builds a pipeline and reproduces a solid source.
#[test]
fn combined_texel_fetch_reproduces_solid_source() {
    let color = [200u8, 120, 30];
    let out = render(COMBINED_TEXEL_FETCH, &solid(color));
    let got = centre_mean(&out);
    let want = [color[0] as f64, color[1] as f64, color[2] as f64];
    assert!(
        dist(got, want) < 8.0,
        "combined texelFetch should reproduce the source color {want:?}, got {got:?}"
    );
}

/// Test 4: the no-op path — the separate-sampler fixture still renders correctly
/// (the transform must not perturb already-separate shaders).
#[test]
fn separate_sampler_still_renders_after_split_is_added() {
    let color = [15u8, 200, 95];
    let out = render(SEPARATE_PASSTHROUGH, &solid(color));
    let got = centre_mean(&out);
    let want = [color[0] as f64, color[1] as f64, color[2] as f64];
    assert!(
        dist(got, want) < 8.0,
        "separate-sampler passthrough must still reproduce {want:?}, got {got:?}"
    );
}
