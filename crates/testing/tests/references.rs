//! Real-RetroArch reference suite (#32, PART B — the Phase-2 fidelity exit gate).
//!
//! ## What this proves — and how it differs from the self-oracle goldens
//!
//! Unlike `golden.rs` (whose `goldens/*.png` were produced by *our own* engine —
//! a self-oracle proving determinism + machinery), the `references/retroarch/*.png`
//! here are **actual RetroArch 1.22.2 screenshots** of the shader output. For each
//! one, this suite renders the SAME preset through our engine over the SAME
//! committed source at the SAME viewport/frame and diffs against the RetroArch
//! capture — proving **fidelity versus RetroArch**, which the goldens deliberately
//! do not. The capture procedure (imageviewer core, the forced `--appendconfig`
//! geometry, frame alignment, calibration) is documented in
//! `docs/golden-image-harness.md` and in each reference's `.toml` sidecar.
//!
//! ## Only MATCHING shaders are listed here
//!
//! Per #32, a reference is committed + asserted ONLY for a shader that achieved a
//! meaningful match. Shaders that diverge (e.g. the `feedback` preset's
//! accumulation, see the doc) are documented as findings, NOT given a passing
//! test. The matching trio captured on this box:
//!
//! | Reference | Preset | Calibrated tol / max_fraction | Observed (engine vs RA) |
//! |---|---|---|---|
//! | `crt-geom` | `crt/crt-geom.slangp` | 4 / 0.001 | max_abs 2, mean 0.001, 0% over tol 4 |
//! | `scanline` | `scanlines/scanline.slangp` | 16 / 0.02 | max_abs 12, mean 0.155, 0.01% over tol 8 |
//! | `ntsc-256px-svideo-scanline` | `ntsc/ntsc-256px-svideo-scanline.slangp` | 24 / 0.05 | max_abs 53, mean 0.166, 0.73% over tol 16 |
//!
//! The near-exact `crt-geom` match is because BOTH renderers ran on llvmpipe
//! (software). The CRT/NTSC tolerances absorb the sub-pixel sampling/rounding
//! differences that remain at high-frequency edges (color-bar boundaries, the
//! checker) while still catching a structural regression (wrong mask, missing
//! curvature, broken NTSC chain).
//!
//! ## Why `#[ignore]` (not run in CI by default)
//!
//! These references were captured on this box's llvmpipe (software Vulkan/GL). CI
//! runs on lavapipe — a *different* software adapter whose sub-pixel rounding
//! differs, so the tight `crt-geom` tolerance in particular may not hold
//! cross-adapter. The references also require the external `slang-shaders` clone
//! (via `SLANG_SHADERS_DIR`, defaulting to the dev path) for the `.slang` sources.
//! So the suite is opt-in:
//!
//! ```bash
//! SLANG_SHADERS_DIR=/path/to/slang-shaders \
//!   WGPU_BACKEND=vulkan cargo test -p testing --test references \
//!   -- --ignored --test-threads=1
//! ```
//!
//! It skips cleanly (a passing no-op) when the corpus dir or a `.slang` source is
//! absent, so running it without the clone never fails.

use std::path::{Path, PathBuf};

use image::RgbaImage;
use source::Frame;
use testing::{diff_image, diff_images, render_preset_to_image};

/// Each committed RetroArch reference under `references/retroarch/`. The `.slangp`
/// path is resolved against the external `slang-shaders` clone; everything else
/// (source, viewport, frame, thresholds) comes from the reference's sidecar.
const REFERENCES: &[&str] = &["crt-geom", "scanline", "ntsc-256px-svideo-scanline"];

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn references_dir() -> PathBuf {
    manifest_dir().join("references").join("retroarch")
}

/// The external `slang-shaders` clone (the `.slang` sources are not vendored).
/// `SLANG_SHADERS_DIR` overrides the dev-box default.
fn slang_shaders_dir() -> PathBuf {
    std::env::var_os("SLANG_SHADERS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/mfunk/Code/slang-shaders"))
}

/// The parsed reference metadata from a `.toml` sidecar. Parsed with a tiny
/// key=value reader (the sidecars are flat `key = value` lines) so the suite needs
/// no TOML dependency.
struct RefMeta {
    preset: String,
    source: String,
    viewport: (u32, u32),
    frame_index: u64,
    tolerance: u8,
    max_fraction: f64,
}

fn parse_sidecar(path: &Path) -> RefMeta {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read sidecar {}: {e}", path.display()));
    let get = |key: &str| -> Option<String> {
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == key {
                    return Some(v.trim().trim_matches('"').to_string());
                }
            }
        }
        None
    };
    let num = |key: &str| -> String {
        get(key).unwrap_or_else(|| panic!("sidecar {} missing key `{key}`", path.display()))
    };
    RefMeta {
        preset: num("preset"),
        source: num("source"),
        viewport: (
            num("viewport_width").parse().unwrap(),
            num("viewport_height").parse().unwrap(),
        ),
        frame_index: num("frame_index").parse().unwrap(),
        tolerance: num("diff_tolerance").parse().unwrap(),
        max_fraction: num("diff_max_fraction").parse().unwrap(),
    }
}

/// Load a PNG (the committed source / reference) into a [`Frame`] / [`RgbaImage`].
fn load_frame(path: &Path) -> Frame {
    let img = image::open(path)
        .unwrap_or_else(|e| panic!("open {}: {e}", path.display()))
        .to_rgba8();
    Frame::new(img.width(), img.height(), img.into_raw())
}

fn artifacts_dir() -> PathBuf {
    manifest_dir()
        .join("..")
        .join("..")
        .join("target")
        .join("reference-artifacts")
}

/// Render `name`'s preset through our engine and diff it against the committed
/// RetroArch reference with the sidecar's calibrated threshold. Skips cleanly
/// (returns) when the corpus dir or the preset's `.slang` source is absent.
fn check_reference(name: &str) {
    let sidecar = references_dir().join(format!("{name}.toml"));
    let reference_png = references_dir().join(format!("{name}.png"));
    let meta = parse_sidecar(&sidecar);

    let preset_path = slang_shaders_dir().join(&meta.preset);
    if !preset_path.exists() {
        eprintln!(
            "skipping reference `{name}`: preset {} not found (set SLANG_SHADERS_DIR to the \
             slang-shaders clone). This is fine.",
            preset_path.display()
        );
        return;
    }

    let source = load_frame(&manifest_dir().join(&meta.source));
    let rendered = render_preset_to_image(&preset_path, &source, meta.viewport, meta.frame_index)
        .unwrap_or_else(|e| panic!("render {name} ({}): {e}", preset_path.display()));

    let reference: RgbaImage = image::open(&reference_png)
        .unwrap_or_else(|e| panic!("open reference {}: {e}", reference_png.display()))
        .to_rgba8();

    let report = diff_images(&rendered, &reference, meta.tolerance, meta.max_fraction);
    if !report.passed {
        let dir = artifacts_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = rendered.save(dir.join(format!("{name}.rendered.png")));
        let _ = diff_image(&rendered, &reference, 8).save(dir.join(format!("{name}.diff.png")));
        panic!(
            "reference `{name}` diverged from RetroArch: max_abs={} mean_abs={:.4} \
             pct_over={:.4} (calibrated tol {}, max_fraction {}). Artifacts in {}.",
            report.max_abs,
            report.mean_abs,
            report.pct_pixels_over_threshold,
            meta.tolerance,
            meta.max_fraction,
            dir.display()
        );
    }
    eprintln!(
        "reference `{name}` MATCHES RetroArch: max_abs={} mean_abs={:.4} pct_over={:.4} \
         (tol {}, max_fraction {})",
        report.max_abs,
        report.mean_abs,
        report.pct_pixels_over_threshold,
        meta.tolerance,
        meta.max_fraction
    );
}

#[test]
#[ignore = "real-RetroArch references captured on this box's llvmpipe; needs the \
            slang-shaders clone (SLANG_SHADERS_DIR) and may not hold cross-adapter on CI"]
fn engine_matches_retroarch_references() {
    for name in REFERENCES {
        check_reference(name);
    }
}
