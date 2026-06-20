//! The **simulated viewport** and its content-rectangle math (#30, Architecture
//! §D, `docs/retroarch-slang-runtime.md` §2/§9).
//!
//! RetroArch renders the final image at an **output resolution** (e.g. 1920×1080)
//! that is generally *not* the source/game resolution, and fits the source into
//! that output either aspect-correct (preserving the source ratio, letterboxing /
//! pillarboxing the remainder) or — with integer-scale on — snapped to the
//! largest whole multiple of the source that fits, again letterboxing the rest
//! with black bars (§9).
//!
//! This module owns the **canonical, pure** computation: [`ViewportConfig`] is
//! the output resolution + integer-scale toggle, and [`ViewportConfig::content_rect`]
//! resolves the source size into the centered [`ViewportRect`] (size + offset)
//! the final image actually occupies within the output. The renderer
//! ([`crate::renderer`]) feeds that rect's *size* into `viewport`-scaled FBO
//! sizing, the final pass's `OutputSize`, and `FinalViewportSize`, and uses the
//! *offset* to place the final image (with black bars) when compositing into the
//! preview pane.
//!
//! Lives in `preview-engine` (not `core-model`) on purpose: the engine
//! deliberately has no compile dependency on `core-model` (the app converts the
//! `core_model::Viewport` schema type to this at the IPC boundary, mirroring how
//! `ScaleType`/`WrapMode` are duplicated — see [`crate::pass`]).

/// The simulated viewport: the output resolution the final pass renders at, plus
/// the integer-scale toggle (#30). Distinct from the preview *pane* size (the
/// read-back/stream target — [`crate::renderer::Renderer::set_viewport`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewportConfig {
    /// Output resolution width in pixels.
    pub width: u32,
    /// Output resolution height in pixels.
    pub height: u32,
    /// When `true`, snap the content to the largest integer multiple of the
    /// source size that fits (§9); else aspect-correct fit (preserve source ratio).
    pub integer_scale: bool,
}

/// The effective **content rectangle** within the output resolution (#30): the
/// pixel size the final image occupies and its top-left offset (centered, so the
/// remainder forms equal black bars). `offset + size` never exceeds the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewportRect {
    /// Content width in pixels (≤ output width).
    pub width: u32,
    /// Content height in pixels (≤ output height).
    pub height: u32,
    /// Left black-bar width (centers the content horizontally).
    pub offset_x: u32,
    /// Top black-bar height (centers the content vertically).
    pub offset_y: u32,
}

impl ViewportConfig {
    /// Resolve this viewport's content rectangle for a given `source` size (#30,
    /// §9), matching RetroArch's fit math:
    ///
    /// - **Integer-scale** (`integer_scale == true`): the content is the largest
    ///   integer multiple of the source that fits the output —
    ///   `n = max(1, min(out_w / src_w, out_h / src_h))` (floored integer
    ///   division; `n ≥ 1`), content `= (n·src_w, n·src_h)`. A source larger than
    ///   the output (where `n` would floor to 0) is bumped to `n = 1` and then
    ///   clamped so the content never exceeds the output.
    /// - **Aspect-fit** (`integer_scale == false`): preserve the source aspect
    ///   ratio, scaling by `s = min(out_w / src_w, out_h / src_h)` (real-valued),
    ///   content `= (round(src_w·s), round(src_h·s))`, clamped to the output —
    ///   this letterboxes (bars top/bottom) or pillarboxes (bars left/right) to
    ///   keep the source ratio.
    ///
    /// In both cases the content is **centered**: each offset is half the
    /// remainder, rounded down (so the left/top bar is the floor and any odd
    /// leftover pixel lands in the right/bottom bar). A zero source dimension is
    /// treated as `1` to avoid a divide-by-zero (a defined, if degenerate, rect).
    pub fn content_rect(&self, source: (u32, u32)) -> ViewportRect {
        // Guard against zero output/source dims so the math never divides by zero
        // and always yields a well-defined (≥1) rect.
        let out_w = self.width.max(1);
        let out_h = self.height.max(1);
        let src_w = source.0.max(1);
        let src_h = source.1.max(1);

        let (content_w, content_h) = if self.integer_scale {
            // Largest integer multiple of the source that fits (§9). `min(...)`
            // bounds the multiple by the tighter axis; `max(1, ...)` guarantees at
            // least 1× even when the source is larger than the output (the clamp
            // below then trims a too-large 1× content back to the output).
            let n = (out_w / src_w).min(out_h / src_h).max(1);
            (src_w * n, src_h * n)
        } else {
            // Aspect-correct fit: scale by the tighter axis ratio so the whole
            // source fits while preserving its ratio (the looser axis letterboxes).
            let s = (out_w as f32 / src_w as f32).min(out_h as f32 / src_h as f32);
            (
                (src_w as f32 * s).round() as u32,
                (src_h as f32 * s).round() as u32,
            )
        };

        // Clamp the content into the output (integer-scale 1× of an oversized
        // source, or a rounding overshoot, must not exceed it) and center it. A
        // clamped-to-zero content is bumped to 1px so the rect stays drawable.
        let content_w = content_w.clamp(1, out_w);
        let content_h = content_h.clamp(1, out_h);
        let offset_x = (out_w - content_w) / 2;
        let offset_y = (out_h - content_h) / 2;

        ViewportRect {
            width: content_w,
            height: content_h,
            offset_x,
            offset_y,
        }
    }
}

/// Where the content was composited in the **pane** and how big it is in
/// **simulated-viewport** pixels — the two rectangles the pixel inspector (#61)
/// needs to turn a PANE coordinate into a SIMULATED-VIEWPORT coordinate.
///
/// The renderer composites the §9 content rect (in output-resolution space) into a
/// `pane_rect` sub-rectangle of the pane (centered, with black letterbox bars when
/// a simulated viewport is active), and the content itself is `content_size`
/// viewport pixels. With NO simulated viewport the pane *is* the content, so
/// `pane_rect` is the whole pane and `content_size` equals the pane size — the
/// mapping is then an identity. Pure (no GPU), so the pane↔viewport transform is
/// unit-testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneMapping {
    /// The content's rectangle in PANE pixel space `(x, y, width, height)` — where
    /// the final image was composited (the rest of the pane is letterbox bars).
    pub pane_rect: (u32, u32, u32, u32),
    /// The content's size in SIMULATED-VIEWPORT pixels `(width, height)` — the §9
    /// content-rect size, the resolution the inspected value is reported against.
    pub content_size: (u32, u32),
}

/// One mapped pixel (#61): whether the pane coordinate landed on the content, and
/// the simulated-viewport pixel it maps to (`(0, 0)` when outside).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MappedPixel {
    /// `true` when the pane coordinate is inside the content rect (a real sample);
    /// `false` when it landed in a letterbox bar (no sample).
    pub inside: bool,
    /// The simulated-viewport pixel X (content-rect space).
    pub x: u32,
    /// The simulated-viewport pixel Y (content-rect space).
    pub y: u32,
}

impl PaneMapping {
    /// Map a PANE pixel `(px, py)` to a SIMULATED-VIEWPORT pixel (#61).
    ///
    /// A pane pixel outside `pane_rect` (a letterbox bar) returns `inside == false`.
    /// Inside, the pane pixel's CENTER is normalized within `pane_rect` and scaled
    /// into the content size, floored to a whole viewport pixel and clamped to the
    /// last valid index. Using the pixel center (`+0.5`) keeps the mapping centered
    /// so the first/last pane pixels map to the first/last viewport pixels rather
    /// than biasing toward the origin.
    pub fn map_pane_pixel(&self, px: u32, py: u32) -> MappedPixel {
        let (rx, ry, rw, rh) = self.pane_rect;
        // Outside the content rect (a letterbox bar) — no sample. A zero-size rect
        // (degenerate) is treated as wholly outside.
        if rw == 0 || rh == 0 || px < rx || py < ry || px >= rx + rw || py >= ry + rh {
            return MappedPixel {
                inside: false,
                x: 0,
                y: 0,
            };
        }
        let (cw, ch) = (self.content_size.0.max(1), self.content_size.1.max(1));
        // Normalize the pane pixel center within the composite rect, scale into the
        // content size, floor, and clamp to the last valid index.
        let u = (px - rx) as f32 + 0.5;
        let v = (py - ry) as f32 + 0.5;
        let vx = ((u / rw as f32) * cw as f32).floor() as u32;
        let vy = ((v / rh as f32) * ch as f32).floor() as u32;
        MappedPixel {
            inside: true,
            x: vx.min(cw - 1),
            y: vy.min(ch - 1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(width: u32, height: u32, integer_scale: bool) -> ViewportConfig {
        ViewportConfig {
            width,
            height,
            integer_scale,
        }
    }

    /// A content rect must always fit inside the output (offset + size ≤ output)
    /// and be at least 1×1 — the universal invariant we assert alongside the
    /// specific dimension checks.
    fn assert_within_output(c: &ViewportConfig, r: &ViewportRect) {
        assert!(r.width >= 1 && r.height >= 1, "content must be ≥1px: {r:?}");
        assert!(
            r.offset_x + r.width <= c.width.max(1),
            "content overflows output width: rect {r:?} in {}x{}",
            c.width,
            c.height
        );
        assert!(
            r.offset_y + r.height <= c.height.max(1),
            "content overflows output height: rect {r:?} in {}x{}",
            c.width,
            c.height
        );
    }

    #[test]
    fn integer_scale_snaps_to_largest_multiple_bounded_by_height() {
        // SNES 256x224 into 1920x1080, integer-scale: 1920/256 = 7, 1080/224 = 4
        // -> n = min(7,4) = 4 -> content 1024x896, centered with letterbox bars.
        let c = cfg(1920, 1080, true);
        let r = c.content_rect((256, 224));
        assert_eq!(
            (r.width, r.height),
            (1024, 896),
            "n=4 multiple of the source"
        );
        // Centered: (1920-1024)/2 = 448, (1080-896)/2 = 92.
        assert_eq!((r.offset_x, r.offset_y), (448, 92), "centered remainder");
        assert_within_output(&c, &r);
    }

    #[test]
    fn integer_scale_bounded_by_width() {
        // A wide source into a square-ish output so WIDTH is the tighter axis.
        // 400x100 into 1000x1000: 1000/400 = 2, 1000/100 = 10 -> n = 2 -> 800x200.
        let c = cfg(1000, 1000, true);
        let r = c.content_rect((400, 100));
        assert_eq!((r.width, r.height), (800, 200), "width-bounded n=2");
        assert_eq!((r.offset_x, r.offset_y), (100, 400));
        assert_within_output(&c, &r);
    }

    #[test]
    fn integer_scale_exact_fit_has_no_bars() {
        // 320x240 into 1280x960: 1280/320 = 4, 960/240 = 4 -> n=4 -> exactly fills.
        let c = cfg(1280, 960, true);
        let r = c.content_rect((320, 240));
        assert_eq!((r.width, r.height), (1280, 960), "exact 4x fill");
        assert_eq!((r.offset_x, r.offset_y), (0, 0), "no letterbox");
        assert_within_output(&c, &r);
    }

    #[test]
    fn integer_scale_oversized_source_clamps_to_one_x() {
        // Source bigger than the output: floor division would give 0 on both axes;
        // the max(1,...) bumps to 1x and the clamp trims the (too-large) 1x content
        // back to the output — content == output, no bars (degenerate but defined).
        let c = cfg(640, 480, true);
        let r = c.content_rect((1280, 1024));
        assert_eq!((r.width, r.height), (640, 480), "1x clamped to the output");
        assert_eq!((r.offset_x, r.offset_y), (0, 0));
        assert_within_output(&c, &r);
    }

    #[test]
    fn aspect_fit_letterboxes_a_wider_output() {
        // 256x224 (≈1.143) into 800x600 (≈1.333): the output is WIDER than the
        // source ratio, so it pillarboxes — fit by height. s = min(800/256,
        // 600/224) = min(3.125, 2.679) = 2.679 -> 256*2.679≈686, 224*2.679=600.
        let c = cfg(800, 600, false);
        let r = c.content_rect((256, 224));
        assert_eq!(r.height, 600, "fit to height (the tighter axis)");
        assert_eq!(r.width, 686, "source ratio preserved: round(256 * 600/224)");
        // Pillarbox: bars are left/right, none top/bottom.
        assert_eq!(r.offset_y, 0, "no top/bottom bar (height-fit)");
        assert_eq!(r.offset_x, (800 - 686) / 2, "centered pillarbox");
        assert_within_output(&c, &r);
        // The source aspect ratio is preserved within rounding.
        let src_ratio = 256.0 / 224.0;
        let content_ratio = r.width as f32 / r.height as f32;
        assert!(
            (src_ratio - content_ratio).abs() < 0.01,
            "aspect preserved: src {src_ratio} vs content {content_ratio}"
        );
    }

    #[test]
    fn aspect_fit_letterboxes_a_taller_output() {
        // 16:9 source (1920x1080) into a 1000x1000 square output: the output is
        // TALLER than the source ratio, so it letterboxes — fit by width. s =
        // min(1000/1920, 1000/1080) = 1000/1920 -> width 1000, height round(1080 *
        // 1000/1920) = round(562.5) = 563 (round-half-to-even/away may give 562/563).
        let c = cfg(1000, 1000, false);
        let r = c.content_rect((1920, 1080));
        assert_eq!(r.width, 1000, "fit to width (the tighter axis)");
        assert!(
            (r.height as i32 - 563).abs() <= 1,
            "source ratio preserved: ~563, got {}",
            r.height
        );
        assert_eq!(r.offset_x, 0, "no left/right bar (width-fit)");
        assert_eq!(r.offset_y, (1000 - r.height) / 2, "centered letterbox");
        assert_within_output(&c, &r);
    }

    #[test]
    fn aspect_fit_exact_ratio_fills_with_no_bars() {
        // Source ratio == output ratio (both 4:3): the fit fills exactly.
        let c = cfg(1024, 768, false);
        let r = c.content_rect((640, 480));
        assert_eq!((r.width, r.height), (1024, 768), "exact fill, same ratio");
        assert_eq!((r.offset_x, r.offset_y), (0, 0), "no bars");
        assert_within_output(&c, &r);
    }

    #[test]
    fn content_never_exceeds_output_for_random_pairs() {
        // The clamp + center invariant must hold for a spread of sizes and both
        // modes, including sources larger than the output and 1px edge cases.
        let outputs = [(1920, 1080), (640, 480), (1, 1), (300, 1000), (1000, 7)];
        let sources = [(256, 224), (1920, 1080), (1, 1), (4000, 10), (33, 999)];
        for &out in &outputs {
            for &src in &sources {
                for integer_scale in [true, false] {
                    let c = cfg(out.0, out.1, integer_scale);
                    let r = c.content_rect(src);
                    assert_within_output(&c, &r);
                }
            }
        }
    }

    #[test]
    fn zero_dims_are_treated_as_one() {
        // A zero source/output dimension must not panic (no divide-by-zero) and
        // yields a defined ≥1 rect.
        let c = cfg(0, 0, true);
        let r = c.content_rect((0, 0));
        assert_within_output(&c, &r);
        let c2 = cfg(640, 480, false);
        let r2 = c2.content_rect((0, 480));
        assert_within_output(&c2, &r2);
    }

    // ---- Pane → simulated-viewport pixel mapping (#61). ----

    #[test]
    fn pane_mapping_identity_when_pane_is_the_content() {
        // No simulated viewport: pane == content, so the pane pixel maps 1:1.
        let m = PaneMapping {
            pane_rect: (0, 0, 100, 80),
            content_size: (100, 80),
        };
        assert_eq!(
            m.map_pane_pixel(0, 0),
            MappedPixel {
                inside: true,
                x: 0,
                y: 0
            }
        );
        assert_eq!(
            m.map_pane_pixel(50, 40),
            MappedPixel {
                inside: true,
                x: 50,
                y: 40
            }
        );
        // The last pane pixel maps to the last viewport pixel (center sampling).
        assert_eq!(
            m.map_pane_pixel(99, 79),
            MappedPixel {
                inside: true,
                x: 99,
                y: 79
            }
        );
    }

    #[test]
    fn pane_mapping_letterbox_bars_report_outside() {
        // Content composited into a centered 60x40 rect inside a 100x80 pane: the
        // bars (everything outside that rect) report `inside == false`.
        let m = PaneMapping {
            pane_rect: (20, 20, 60, 40),
            content_size: (60, 40),
        };
        // Top-left corner of the pane is in the bar.
        assert!(!m.map_pane_pixel(0, 0).inside, "top-left bar");
        // Just left of the content rect.
        assert!(!m.map_pane_pixel(19, 30).inside, "left bar");
        // Just past the right edge (x = 20 + 60 = 80 is the first bar pixel).
        assert!(!m.map_pane_pixel(80, 30).inside, "right bar");
        // Inside the content rect.
        let inner = m.map_pane_pixel(20, 20);
        assert!(inner.inside, "content pixel is inside");
        assert_eq!((inner.x, inner.y), (0, 0), "first content pixel");
    }

    #[test]
    fn pane_mapping_upscales_pane_into_a_larger_viewport() {
        // A small pane rect (50x40) showing a larger simulated viewport (200x160):
        // each pane pixel maps to a 4x viewport pixel; the first/last span the
        // content range.
        let m = PaneMapping {
            pane_rect: (0, 0, 50, 40),
            content_size: (200, 160),
        };
        assert_eq!(
            m.map_pane_pixel(0, 0),
            MappedPixel {
                inside: true,
                x: 2,
                y: 2
            },
            "pane (0,0) center maps into the first 4x cell"
        );
        // Mid-pane maps to mid-viewport.
        let mid = m.map_pane_pixel(25, 20);
        assert_eq!((mid.x, mid.y), (102, 82), "mid pane -> mid viewport");
        // The last pane pixel maps to (clamped) the last viewport pixel.
        let last = m.map_pane_pixel(49, 39);
        assert_eq!((last.x, last.y), (198, 158), "last pane pixel near the end");
    }

    #[test]
    fn pane_mapping_downscales_pane_from_a_smaller_viewport() {
        // A large pane rect (200x160) showing a smaller viewport (50x40): several
        // pane pixels collapse onto one viewport pixel, and the index never exceeds
        // the content bounds.
        let m = PaneMapping {
            pane_rect: (0, 0, 200, 160),
            content_size: (50, 40),
        };
        assert_eq!(
            m.map_pane_pixel(0, 0),
            MappedPixel {
                inside: true,
                x: 0,
                y: 0
            }
        );
        let last = m.map_pane_pixel(199, 159);
        assert_eq!((last.x, last.y), (49, 39), "clamped to the last index");
        assert!(last.x < 50 && last.y < 40, "never exceeds content bounds");
    }

    #[test]
    fn pane_mapping_zero_size_rect_is_outside() {
        let m = PaneMapping {
            pane_rect: (0, 0, 0, 0),
            content_size: (10, 10),
        };
        assert!(!m.map_pane_pixel(0, 0).inside, "degenerate rect -> outside");
    }

    #[test]
    fn offset_centers_the_remainder_floor() {
        // An odd remainder puts the floor in the left/top bar (the extra pixel in
        // the right/bottom bar). Integer-scale a 30x30 source into 95x95: n =
        // floor(95/30) = 3 -> content 90x90, remainder 5 -> offset floor(5/2) = 2
        // (the left/top bar is 2px, the right/bottom bar is 3px).
        let c = cfg(95, 95, true);
        let r = c.content_rect((30, 30));
        assert_eq!((r.width, r.height), (90, 90), "n=3 multiple");
        assert_eq!(
            (r.offset_x, r.offset_y),
            (2, 2),
            "floor-centered odd remainder"
        );
        // The right/bottom bar gets the extra pixel: 95 - (2 + 90) = 3.
        assert_eq!(
            c.width - (r.offset_x + r.width),
            3,
            "extra px in the far bar"
        );
        assert_within_output(&c, &r);
    }
}
