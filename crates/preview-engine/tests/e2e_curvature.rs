//! Phase 1 exit criterion — full end-to-end slice, headless (NO webview).
//!
//! Proves the risky toolchain works in one shot:
//!   committed PNG  -> `source::load_image`
//!                  -> `slang_compile::compile_slang`  (slang -> glslang -> SPIR-V)
//!                  -> `Renderer` single offscreen wgpu pass with builtins/params
//!                  -> render-to-smaller (GPU downsample) -> read-back Frame.
//!
//! It runs a real curvature/warp shader and a passthrough control over a 128x128
//! source rendered into a 64x64 pane (the downsample path), then asserts the
//! warp is real and corner-localized while the control reproduces the source.
//!
//! GPU-filtering robustness: every comparison is over *region averages* with
//! generous tolerances, never exact single pixels — wgpu's bilinear filtering
//! and the software-vs-hardware adapter differ at the pixel level.
//!
//! CRITICAL: this test does NOT assert byte-equality against the committed
//! `curvature_reference.png`. That reference is documentation / a Phase-2
//! golden-suite seed only: CI renders on lavapipe (software Vulkan) and will not
//! byte-match the NVIDIA box that produced the committed PNG. The structural
//! assertions below are the real gate.

use std::path::{Path, PathBuf};

use preview_engine::Renderer;
use source::Frame;

const SRC_W: u32 = 128;
const SRC_H: u32 = 128;
// Output pane is smaller than the source -> exercises the GPU downsample path.
const OUT_W: u32 = 64;
const OUT_H: u32 = 64;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn artifacts_dir() -> PathBuf {
    // `target/` is gitignored; the snapshot lives at a deterministic path so CI
    // can upload it as an artifact.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("e2e-artifacts")
}

/// Compile a `.slang` fixture (its directory is the include base_dir).
fn compile_fixture(name: &str) -> slang_compile::CompiledShader {
    let dir = fixtures_dir();
    let path = dir.join(name);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()));
    slang_compile::compile_slang(&src, Some(&dir)).unwrap_or_else(|e| panic!("compile {name}: {e}"))
}

/// Run one compiled shader over `source` into an `OUT_W x OUT_H` pane.
fn render(shader: &slang_compile::CompiledShader, src: &Frame) -> Frame {
    let mut r = Renderer::new(OUT_W, OUT_H).expect("wgpu device (set WGPU_BACKEND/VK_ICD on CI)");
    r.set_source(src);
    r.set_shader(shader);
    r.render().expect("render");
    r.read_back().expect("read back")
}

/// Mean RGB over a rectangular region `[x0,x1) x [y0,y1)` of an RGBA8 frame.
fn region_mean(f: &Frame, x0: u32, y0: u32, x1: u32, y1: u32) -> [f64; 3] {
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

/// Euclidean distance between two mean-RGB triples (in 0..=255 units).
fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d0 = a[0] - b[0];
    let d1 = a[1] - b[1];
    let d2 = a[2] - b[2];
    (d0 * d0 + d1 * d1 + d2 * d2).sqrt()
}

/// Whole-frame mean absolute per-channel difference.
fn frame_mad(a: &Frame, b: &Frame) -> f64 {
    assert_eq!(a.rgba.len(), b.rgba.len());
    let sum: u64 = a
        .rgba
        .iter()
        .zip(&b.rgba)
        .map(|(x, y)| x.abs_diff(*y) as u64)
        .sum();
    sum as f64 / a.rgba.len() as f64
}

fn save_png(f: &Frame, path: &Path) {
    let img: image::RgbaImage =
        image::RgbaImage::from_raw(f.width, f.height, f.rgba.clone()).expect("rgba -> image");
    img.save(path)
        .unwrap_or_else(|e| panic!("save {}: {e}", path.display()));
}

#[test]
fn e2e_curvature_over_test_image() {
    // ---- 1. Load the committed source PNG (the full source path). ----------
    let src_path = fixtures_dir().join("test_source.png");
    let source = source::load_image(&src_path)
        .unwrap_or_else(|e| panic!("load {}: {e}", src_path.display()));
    assert_eq!((source.width, source.height), (SRC_W, SRC_H));

    // ---- 2. Compile + render both shaders (downsample 128 -> 64). ----------
    let passthrough = compile_fixture("passthrough.slang");
    let curvature = compile_fixture("curvature.slang");
    let pass_out = render(&passthrough, &source);
    let warp_out = render(&curvature, &source);
    assert_eq!((pass_out.width, pass_out.height), (OUT_W, OUT_H));
    assert_eq!((warp_out.width, warp_out.height), (OUT_W, OUT_H));

    // A CPU box-downsample of the source to the output pane, as a reference for
    // the passthrough control (region-averaged, so the exact filter taps and
    // any half-texel offset wash out).
    let down = box_downsample(&source, OUT_W, OUT_H);

    // ---- 3. Passthrough control reproduces the (downsampled) source. -------
    // Compare central + the four quadrant-center regions (avoid the very edges
    // where bilinear clamp + grid lines are noisiest). Generous tolerance for
    // GPU-vs-CPU filtering differences.
    const PASS_TOL: f64 = 22.0;
    for (rx, ry) in [(16, 16), (16, 48), (48, 16), (48, 48), (32, 32)] {
        let r = (rx - 6, ry - 6, rx + 6, ry + 6);
        let got = region_mean(&pass_out, r.0, r.1, r.2, r.3);
        let want = region_mean(&down, r.0, r.1, r.2, r.3);
        let d = dist(got, want);
        assert!(
            d <= PASS_TOL,
            "passthrough region ({rx},{ry}) drifted from source: dist {d:.1} > {PASS_TOL} (got {got:?} want {want:?})"
        );
    }

    // ---- 4. Curvature warps the CORNERS notably vs. passthrough. -----------
    // The barrel warp pushes the framebuffer corners outside the source, so they
    // read black while the passthrough corners are bright quadrant colors. Each
    // 12x12 corner region must differ a lot; the center must stay ~unchanged.
    const CORNER: u32 = 12;
    let corners = [
        (0, 0, CORNER, CORNER),
        (OUT_W - CORNER, 0, OUT_W, CORNER),
        (0, OUT_H - CORNER, CORNER, OUT_H),
        (OUT_W - CORNER, OUT_H - CORNER, OUT_W, OUT_H),
    ];
    const CORNER_MIN_DIFF: f64 = 60.0;
    for (i, &(x0, y0, x1, y1)) in corners.iter().enumerate() {
        let pm = region_mean(&pass_out, x0, y0, x1, y1);
        let wm = region_mean(&warp_out, x0, y0, x1, y1);
        let d = dist(pm, wm);
        assert!(
            d >= CORNER_MIN_DIFF,
            "curvature corner {i} barely changed: dist {d:.1} < {CORNER_MIN_DIFF} (pass {pm:?} warp {wm:?})"
        );
        // The warped corner should be near-black (outside-source -> clear color).
        let wbright = wm[0] + wm[1] + wm[2];
        assert!(
            wbright < 90.0,
            "curvature corner {i} should be ~black, got sum {wbright:.1} ({wm:?})"
        );
    }

    // Center region (~undistorted) stays close between control and warp.
    let pc = region_mean(&pass_out, 26, 26, 38, 38);
    let wc = region_mean(&warp_out, 26, 26, 38, 38);
    const CENTER_TOL: f64 = 30.0;
    let dc = dist(pc, wc);
    assert!(
        dc <= CENTER_TOL,
        "curvature center should stay ~unchanged: dist {dc:.1} > {CENTER_TOL} (pass {pc:?} warp {wc:?})"
    );

    // ---- 5. Guard against no-ops: warp output differs from the source. -----
    let mad = frame_mad(&warp_out, &down);
    assert!(
        mad >= 5.0,
        "curvature output is suspiciously close to the source (mad {mad:.2}) — possible no-op"
    );

    // ---- 6. Write the live snapshot artifact + seed the committed reference.
    let art_dir = artifacts_dir();
    std::fs::create_dir_all(&art_dir).expect("create e2e-artifacts dir");
    save_png(&warp_out, &art_dir.join("curvature_output.png"));
    save_png(&pass_out, &art_dir.join("passthrough_output.png"));

    // Seed the committed reference PNG only if it is missing, so the first run on
    // this box produces it deterministically without ever failing CI on a
    // byte-mismatch (see the module-level CRITICAL note).
    let reference = fixtures_dir().join("curvature_reference.png");
    if !reference.exists() {
        save_png(&warp_out, &reference);
    }
}

/// Deterministic CPU box-downsample of an RGBA8 frame to `dw x dh` (averages the
/// source block mapping to each target pixel). Reference for the passthrough
/// region comparisons; not a pixel-exact model of the GPU's bilinear sampler.
fn box_downsample(src: &Frame, dw: u32, dh: u32) -> Frame {
    let mut out = vec![0u8; (dw * dh * 4) as usize];
    for ty in 0..dh {
        for tx in 0..dw {
            let sx0 = tx * src.width / dw;
            let sx1 = ((tx + 1) * src.width / dw).max(sx0 + 1);
            let sy0 = ty * src.height / dh;
            let sy1 = ((ty + 1) * src.height / dh).max(sy0 + 1);
            let mut acc = [0u64; 4];
            let mut n = 0u64;
            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    let i = ((sy * src.width + sx) * 4) as usize;
                    for (c, a) in acc.iter_mut().enumerate() {
                        *a += src.rgba[i + c] as u64;
                    }
                    n += 1;
                }
            }
            let o = ((ty * dw + tx) * 4) as usize;
            for (c, a) in acc.iter().enumerate() {
                out[o + c] = (a / n) as u8;
            }
        }
    }
    Frame::new(dw, dh, out)
}
