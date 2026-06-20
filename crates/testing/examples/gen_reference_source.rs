//! Generate the committed, deterministic source image used by BOTH RetroArch and
//! our engine when capturing real-RetroArch reference images (#32, PART B).
//!
//! The Phase-2 fidelity exit gate compares a shader rendered in **RetroArch** to
//! the same shader rendered through our [`testing::render_preset_to_image`]. For
//! that to be apples-to-apples, both renderers must consume the *same* source
//! pixels. RetroArch loads a still PNG as content via the `imageviewer` core; our
//! engine loads the same PNG into a [`source::Frame`] still pump. This example
//! writes that PNG so it is reproducible and reviewable in version control.
//!
//! Run (writes `crates/testing/references/src/testcard_320x240.png`):
//!
//! ```bash
//! cargo run -p testing --example gen_reference_source
//! ```
//!
//! The pattern is a 320×240 RGBA test card chosen to give CRT/NTSC/blur shaders
//! real signal to work on:
//! * top half — eight vertical color bars (SMPTE-ish order) so chroma/mask shaders
//!   have saturated primaries and secondaries;
//! * bottom half — a horizontal luma gradient with a fine 4×4 checker overlay, so
//!   high-frequency response (scanlines, sharpening, ringing) is exercised.
//!
//! It is fully deterministic (a pure function of pixel coordinates), so re-running
//! reproduces the committed bytes.

use std::path::Path;

/// The fixed reference source resolution. Small enough to keep references tiny,
/// large enough that 8 color bars and a checker are well-resolved.
const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;

fn main() {
    let out = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("references")
        .join("src")
        .join("testcard_320x240.png");
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).expect("create references/src dir");
    }

    let img = build_testcard();
    img.save(&out)
        .unwrap_or_else(|e| panic!("write {}: {e}", out.display()));
    println!("wrote {} ({WIDTH}x{HEIGHT})", out.display());
}

/// Build the deterministic test card (also used by tests so the pattern has a
/// single source of truth).
fn build_testcard() -> image::RgbaImage {
    let mut img = image::RgbaImage::new(WIDTH, HEIGHT);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let px = if y < HEIGHT / 2 {
                // Eight vertical color bars.
                const BARS: [[u8; 3]; 8] = [
                    [255, 255, 255],
                    [255, 255, 0],
                    [0, 255, 255],
                    [0, 255, 0],
                    [255, 0, 255],
                    [255, 0, 0],
                    [0, 0, 255],
                    [16, 16, 16],
                ];
                let bar = (x * 8 / WIDTH) as usize;
                BARS[bar.min(7)]
            } else {
                // Luma gradient + fine checker.
                let grad = (x * 255 / (WIDTH - 1)) as u8;
                let checker = (x / 4 + y / 4) % 2;
                let v = if checker == 0 {
                    grad
                } else {
                    grad.saturating_sub(40)
                };
                [v, v, v]
            };
            img.put_pixel(x, y, image::Rgba([px[0], px[1], px[2], 255]));
        }
    }
    img
}
