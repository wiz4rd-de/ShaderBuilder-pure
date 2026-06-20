//! Image diff for the golden suite (#32, Architecture §G.3): a numeric per-pixel
//! metric with a pass/fail verdict, plus an amplified visual diff artifact to
//! write on failure.
//!
//! The metric is intentionally simple and explainable (a CRT/NTSC reference diff
//! must be human-auditable): per-channel **absolute** difference, summarized as
//!
//! * `max_abs` — the single largest per-channel absolute difference (0..=255),
//! * `mean_abs` — the mean per-channel absolute difference over all channels,
//! * `pct_pixels_over_threshold` — the fraction (0.0..=1.0) of pixels whose
//!   **max-channel** absolute difference exceeds `tolerance`.
//!
//! `passed` is `pct_pixels_over_threshold <= max_fraction`. This "fraction of
//! pixels that differ by more than a per-channel tolerance" is a deliberately
//! lenient perceptual proxy: GPU bilinear filtering and a software-vs-hardware
//! adapter perturb many pixels by a few units (which `tolerance` absorbs) while a
//! real regression moves a *large fraction* of pixels well past it. A size
//! mismatch is an automatic fail (the images are incomparable).
//!
//! Thresholds are **parameters**, not constants: [`diff_images`] takes both the
//! per-channel `tolerance` and the `max_fraction`, and the doc
//! `docs/golden-image-harness.md` records the rationale for the values the tests
//! use.

use image::RgbaImage;

/// The result of comparing two images (#32). All fields are reported (not just
/// the verdict) so a failing CI run shows *how far* off the render is, and a
/// human can decide whether to re-baseline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiffReport {
    /// The largest single per-channel absolute difference (0..=255). `0` ⇒ the
    /// images are byte-identical.
    pub max_abs: u8,
    /// The mean per-channel absolute difference across every channel of every
    /// pixel (RGBA), in 0.0..=255.0.
    pub mean_abs: f64,
    /// The fraction (0.0..=1.0) of pixels whose max-channel absolute difference
    /// exceeds the `tolerance` passed to [`diff_images`].
    pub pct_pixels_over_threshold: f64,
    /// Whether the comparison passed: a matching size **and**
    /// `pct_pixels_over_threshold <= max_fraction`. A size mismatch is always
    /// `false`.
    pub passed: bool,
}

impl DiffReport {
    /// The report for two images that cannot be compared (different sizes): a
    /// guaranteed fail with saturated metrics, so a size regression is obvious.
    fn size_mismatch() -> Self {
        DiffReport {
            max_abs: u8::MAX,
            mean_abs: f64::from(u8::MAX),
            pct_pixels_over_threshold: 1.0,
            passed: false,
        }
    }
}

/// Compare two RGBA8 images and produce a [`DiffReport`] (#32).
///
/// * `tolerance` — the per-channel absolute difference a pixel may show before it
///   counts toward `pct_pixels_over_threshold` (a pixel is "over" iff its
///   *max-channel* abs diff is strictly greater than `tolerance`).
/// * `max_fraction` — the largest `pct_pixels_over_threshold` that still passes
///   (0.0..=1.0). `0.0` means "no pixel may exceed the tolerance".
///
/// A size mismatch returns [`DiffReport::size_mismatch`] (an automatic fail).
pub fn diff_images(a: &RgbaImage, b: &RgbaImage, tolerance: u8, max_fraction: f64) -> DiffReport {
    if a.dimensions() != b.dimensions() {
        return DiffReport::size_mismatch();
    }

    let a = a.as_raw();
    let b = b.as_raw();
    debug_assert_eq!(a.len(), b.len());

    let mut max_abs: u8 = 0;
    let mut sum_abs: u64 = 0;
    let mut pixels_over: u64 = 0;
    let pixel_count = (a.len() / 4) as u64;

    for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        let mut pixel_max: u8 = 0;
        for c in 0..4 {
            let d = pa[c].abs_diff(pb[c]);
            sum_abs += d as u64;
            pixel_max = pixel_max.max(d);
        }
        max_abs = max_abs.max(pixel_max);
        if pixel_max > tolerance {
            pixels_over += 1;
        }
    }

    let channel_count = (pixel_count * 4).max(1);
    let mean_abs = sum_abs as f64 / channel_count as f64;
    let pct_pixels_over_threshold = if pixel_count == 0 {
        0.0
    } else {
        pixels_over as f64 / pixel_count as f64
    };

    DiffReport {
        max_abs,
        mean_abs,
        pct_pixels_over_threshold,
        passed: pct_pixels_over_threshold <= max_fraction,
    }
}

/// Produce a **visual diff artifact** (#32): an RGBA8 image whose RGB encodes the
/// amplified absolute per-channel difference and whose alpha is opaque. Identical
/// inputs give a black image; a divergence lights up where (and how badly) the
/// images differ, so a failing CI run can upload a glanceable picture.
///
/// `amplify` multiplies each absolute difference before clamping to 255 (e.g.
/// `8` makes a 1-unit difference a visible 8). A size mismatch returns a 1×1
/// magenta marker (there is no meaningful per-pixel overlay).
pub fn diff_image(a: &RgbaImage, b: &RgbaImage, amplify: u16) -> RgbaImage {
    if a.dimensions() != b.dimensions() {
        // No per-pixel overlay possible — a single magenta marker pixel.
        return RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 255, 255]));
    }
    let (w, h) = a.dimensions();
    let a = a.as_raw();
    let b = b.as_raw();
    let mut out = Vec::with_capacity(a.len());
    for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        for c in 0..3 {
            let amplified = pa[c].abs_diff(pb[c]) as u16 * amplify;
            out.push(amplified.min(255) as u8);
        }
        out.push(255); // opaque
    }
    RgbaImage::from_raw(w, h, out).expect("diff payload fits the image dimensions")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, image::Rgba(rgba))
    }

    #[test]
    fn identical_images_have_zero_diff_and_pass() {
        let a = solid(4, 4, [10, 20, 30, 255]);
        let b = solid(4, 4, [10, 20, 30, 255]);
        let r = diff_images(&a, &b, 0, 0.0);
        assert_eq!(r.max_abs, 0);
        assert_eq!(r.mean_abs, 0.0);
        assert_eq!(r.pct_pixels_over_threshold, 0.0);
        assert!(r.passed, "identical images must pass even at tolerance 0");
    }

    #[test]
    fn known_constant_offset_gives_expected_metrics() {
        // Every pixel: R differs by 5, G by 0, B by 0, A by 0. So:
        //   max_abs = 5
        //   mean_abs = 5 / 4 channels = 1.25
        //   every pixel's max-channel diff is 5.
        let a = solid(2, 2, [100, 50, 50, 255]);
        let b = solid(2, 2, [105, 50, 50, 255]);

        let r = diff_images(&a, &b, 0, 0.0);
        assert_eq!(r.max_abs, 5);
        assert!((r.mean_abs - 1.25).abs() < 1e-9, "mean_abs {}", r.mean_abs);
        // tolerance 0: a 5-unit diff is "over" for ALL pixels -> 100%.
        assert_eq!(r.pct_pixels_over_threshold, 1.0);
        assert!(!r.passed, "all pixels over tolerance 0 must fail");

        // tolerance 5: a diff of exactly 5 is NOT strictly greater -> 0% over.
        let r2 = diff_images(&a, &b, 5, 0.0);
        assert_eq!(r2.pct_pixels_over_threshold, 0.0);
        assert!(r2.passed, "a 5-diff within tolerance 5 must pass");
    }

    #[test]
    fn fraction_over_threshold_is_counted_per_pixel() {
        // A 2x2 image: change exactly ONE of the four pixels by a large amount.
        let a = solid(2, 2, [0, 0, 0, 255]);
        let mut b = a.clone();
        b.put_pixel(0, 0, image::Rgba([200, 0, 0, 255]));

        // tolerance 10: one of four pixels exceeds it -> 0.25.
        let r = diff_images(&a, &b, 10, 0.0);
        assert_eq!(r.max_abs, 200);
        assert!((r.pct_pixels_over_threshold - 0.25).abs() < 1e-9);
        assert!(!r.passed, "0.25 > max_fraction 0.0 must fail");

        // The same diff passes when max_fraction allows a quarter of the pixels.
        let r2 = diff_images(&a, &b, 10, 0.25);
        assert!(r2.passed, "0.25 <= max_fraction 0.25 must pass");
    }

    #[test]
    fn size_mismatch_is_an_automatic_fail() {
        let a = solid(4, 4, [0, 0, 0, 255]);
        let b = solid(4, 5, [0, 0, 0, 255]);
        let r = diff_images(&a, &b, 255, 1.0);
        assert!(!r.passed, "different sizes must never pass");
        assert_eq!(r.max_abs, u8::MAX);
        assert_eq!(r.pct_pixels_over_threshold, 1.0);
    }

    #[test]
    fn diff_image_is_black_for_identical_inputs() {
        let a = solid(3, 3, [12, 34, 56, 255]);
        let d = diff_image(&a, &a, 8);
        assert_eq!(d.dimensions(), (3, 3));
        for px in d.pixels() {
            assert_eq!(px.0, [0, 0, 0, 255], "no difference should be pure black");
        }
    }

    #[test]
    fn diff_image_amplifies_and_clamps_the_difference() {
        let a = solid(1, 1, [10, 10, 10, 255]);
        let b = solid(1, 1, [13, 50, 10, 255]); // R diff 3, G diff 40, B diff 0
        let d = diff_image(&a, &b, 8);
        let px = d.get_pixel(0, 0).0;
        assert_eq!(px[0], 24, "R: 3 * 8 = 24");
        assert_eq!(px[1], 255, "G: 40 * 8 = 320 -> clamped to 255");
        assert_eq!(px[2], 0, "B: no difference");
        assert_eq!(px[3], 255, "alpha is opaque");
    }

    #[test]
    fn diff_image_on_size_mismatch_is_a_marker() {
        let a = solid(2, 2, [0, 0, 0, 255]);
        let b = solid(3, 3, [0, 0, 0, 255]);
        let d = diff_image(&a, &b, 8);
        assert_eq!(d.dimensions(), (1, 1));
        assert_eq!(d.get_pixel(0, 0).0, [255, 0, 255, 255], "magenta marker");
    }
}
