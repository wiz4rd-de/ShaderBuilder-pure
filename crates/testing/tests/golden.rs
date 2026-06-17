//! Self-oracle golden suite + determinism gate (#32, Architecture §G.2/§G.3).
//!
//! ## What runs here
//!
//! For each committed fixture under `fixtures/`, this:
//!   1. renders it headlessly through [`testing::render_preset_to_image`] over a
//!      FIXED source frame, viewport, and frame index, and
//!   2. diffs the result against a committed golden PNG under `goldens/`.
//!
//! Plus a **determinism** test: rendering the same fixture twice yields
//! byte-identical output (the determinism half of the acceptance criteria).
//!
//! ## The goldens are a SELF-ORACLE, not RetroArch references
//!
//! `goldens/*.png` were produced by THIS engine (re-baseline command below), not
//! captured from RetroArch. Diffing them proves determinism + that the whole
//! compile→chain→feedback→LUT→render→read-back→diff machinery works. They do NOT
//! prove fidelity versus RetroArch — that needs real reference captures, which
//! cannot be produced in this headless environment (see
//! `docs/golden-image-harness.md`, "Manual gate").
//!
//! ## Re-baselining
//!
//! When a render change is INTENTIONAL, regenerate the goldens with:
//!
//! ```text
//! UPDATE_GOLDEN=1 WGPU_BACKEND=vulkan cargo test -p testing --test golden -- --test-threads=1
//! ```
//!
//! This rewrites every `goldens/*.png` from the current engine output and the
//! comparison is skipped (the test passes by construction). Review the PNG diff
//! before committing. Without the env var the goldens are READ-ONLY oracles.
//!
//! ## Adapter tolerance
//!
//! The diff uses a per-channel tolerance + a max-fraction-over-threshold (NOT
//! exact equality): the same engine on a DIFFERENT GPU adapter perturbs many
//! pixels by a few units (bilinear filtering, software-vs-hardware rounding). The
//! thresholds below are generous enough to absorb that yet tight enough to catch
//! a real regression. The byte-exact guarantee is asserted only for the
//! determinism test (same adapter, same process).

use std::path::{Path, PathBuf};

use image::RgbaImage;
use source::Frame;
use testing::{diff_image, diff_images, render_preset_to_image};

/// Fixed viewport every golden renders at. Small + even so the quadrant/center
/// assertions in the fixtures are clean.
const VIEWPORT: (u32, u32) = (32, 32);

/// Per-channel tolerance: a pixel counts as "different" only if a channel is off
/// by more than this. Absorbs adapter/filtering noise (a few units) while a real
/// regression moves channels far more.
const TOLERANCE: u8 = 12;

/// Max fraction of pixels allowed over `TOLERANCE` before the diff fails. ~6% of
/// pixels may wobble (edges, the alias x2 downsample boundary) on a different
/// adapter; a genuine regression flips a much larger share.
const MAX_FRACTION: f64 = 0.06;

/// Amplification for the visual diff artifact written on failure.
const DIFF_AMPLIFY: u16 = 8;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn goldens_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("goldens")
}

fn artifacts_dir() -> PathBuf {
    // `target/` is gitignored; failure diffs land at a deterministic path so CI
    // can upload them as an artifact.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("golden-artifacts")
}

/// The fixed source frame every golden renders over: a deterministic 8x8 RGBA8
/// pattern (a left-red / right-green split with a vertical blue ramp), so the
/// source is non-trivial yet tiny and reproducible. The CONTENT is what the
/// fixtures sample; it never changes across runs.
fn fixed_source() -> Frame {
    let (w, h) = (8u32, 8u32);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let r = if x < w / 2 { 200 } else { 20 };
            let g = if x < w / 2 { 20 } else { 200 };
            let b = (y * 255 / (h - 1)) as u8;
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
    }
    Frame::new(w, h, rgba)
}

/// Whether the suite is in re-baseline mode (`UPDATE_GOLDEN=1`).
fn update_mode() -> bool {
    std::env::var("UPDATE_GOLDEN").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

/// Render a fixture preset over the fixed source at a fixed frame index.
fn render_fixture(rel: &str, frame_index: u64) -> RgbaImage {
    let path = fixtures_dir().join(rel);
    let src = fixed_source();
    render_preset_to_image(&path, &src, VIEWPORT, frame_index)
        .unwrap_or_else(|e| panic!("render {rel} @ frame {frame_index}: {e}"))
}

/// Render `rel`, then either rewrite its golden (UPDATE_GOLDEN) or diff against
/// the committed golden, writing an amplified diff artifact on failure.
fn golden_check(name: &str, rel: &str, frame_index: u64) {
    let rendered = render_fixture(rel, frame_index);
    let golden_path = goldens_dir().join(format!("{name}.png"));

    if update_mode() {
        std::fs::create_dir_all(goldens_dir()).expect("create goldens dir");
        rendered
            .save(&golden_path)
            .unwrap_or_else(|e| panic!("write golden {}: {e}", golden_path.display()));
        eprintln!("UPDATE_GOLDEN: rewrote {}", golden_path.display());
        return;
    }

    let golden = image::open(&golden_path)
        .unwrap_or_else(|e| {
            panic!(
                "open golden {} ({e}). Re-baseline with \
                 UPDATE_GOLDEN=1 cargo test -p testing --test golden -- --test-threads=1",
                golden_path.display()
            )
        })
        .to_rgba8();

    let report = diff_images(&rendered, &golden, TOLERANCE, MAX_FRACTION);
    if !report.passed {
        // Write the rendered output + an amplified diff for inspection on CI.
        let dir = artifacts_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = rendered.save(dir.join(format!("{name}.rendered.png")));
        let _ =
            diff_image(&rendered, &golden, DIFF_AMPLIFY).save(dir.join(format!("{name}.diff.png")));
        panic!(
            "golden {name} diverged: max_abs={} mean_abs={:.3} pct_over={:.4} (tol {TOLERANCE}, \
             max_fraction {MAX_FRACTION}). Artifacts in {}. If intentional, re-baseline with \
             UPDATE_GOLDEN=1.",
            report.max_abs,
            report.mean_abs,
            report.pct_pixels_over_threshold,
            dir.display()
        );
    }
}

// ---- The three feature-exercising goldens (#32 acceptance: a multi-pass chain,
// a feedback shader, and a LUT preset — the LOCAL analogues of the CRT-Royale /
// NTSC / feedback exit-criteria trio, see docs/golden-image-harness.md). ----

#[test]
fn golden_multipass_chain() {
    // scaleN + aliasN + <alias>/Original reference. Frame 0 is enough (no
    // feedback/history dependence).
    golden_check("multipass", "multipass/multipass.slangp", 0);
}

#[test]
fn golden_feedback_accumulate() {
    // PassFeedback0 self-feedback. Render at frame 3 so several accumulation
    // steps have run (out_3 = Source*(1 - 0.5^4)) — proves the double-buffer
    // advanced deterministically, not just the cold first frame.
    golden_check("feedback", "feedback/feedback.slangp", 3);
}

#[test]
fn golden_lut_lookup() {
    // textures= LUT bound by name and sampled across the screen.
    golden_check("lut", "lut/lut.slangp", 0);
}

// ---- Determinism: same input -> byte-identical output (acceptance criterion). ----

#[test]
fn render_is_deterministic_byte_for_byte() {
    let src = fixed_source();
    let path = fixtures_dir().join("feedback/feedback.slangp");
    // Feedback is the most state-dependent fixture, so it is the strongest
    // determinism probe (history + double-buffer must advance identically).
    let a = render_preset_to_image(&path, &src, VIEWPORT, 3).expect("render a");
    let b = render_preset_to_image(&path, &src, VIEWPORT, 3).expect("render b");
    assert_eq!(
        a.dimensions(),
        b.dimensions(),
        "two renders must agree on size"
    );
    assert_eq!(
        a.as_raw(),
        b.as_raw(),
        "same (preset, source, viewport, frame_index) must give byte-identical bytes"
    );
}
