//! Pixel-inspector readback result (#61, Spec §8.5).
//!
//! The preview pane lets the user hover/click a pixel and read its value. The
//! request is a PANE coordinate; the engine maps it through the simulated
//! viewport's content rect (§9 aspect-fit / integer-scale, [`crate::Viewport`])
//! to a SIMULATED-VIEWPORT pixel, reads that pixel back from the offscreen target
//! ON DEMAND (never per frame — `read_back` blocks the render thread), and returns
//! this typed result.
//!
//! It lives in `core-model` (the one shared serde + `#[ts(export)]` schema, §A) so
//! the app's Tauri command and the React overlay share one shape that can never
//! drift — like [`crate::engine::EngineEvent`].
//!
//! The returned `rgba` is the value **as stored in the offscreen target** before
//! any pane downsampling, normalized to `0..1` (a byte/255 for the current
//! `Rgba8Unorm` target). The `format` tag records the offscreen format so the
//! frontend can label HDR/extended values once float targets are wired (today the
//! target is always `Rgba8Unorm`, so values stay in `0..1`; the sRGB/linear and
//! 0-255/0-1 display toggles are a FRONTEND concern and never change this raw
//! readback). The `inside` flag distinguishes a real content pixel from a pane
//! coordinate that landed in a letterbox bar (no sample).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The offscreen target's pixel format, reported alongside a [`PixelSample`] so the
/// frontend can label the readback (#61). Today the target is always
/// [`Rgba8Unorm`](PixelFormat::Rgba8Unorm); the float/srgb variants are the
/// documented hook for when higher-precision targets are wired (Spec §8.5) so the
/// inspector reports HDR/extended values rather than faking them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PixelFormat {
    /// 8-bit per channel, linear unorm — values are exactly `byte / 255` in `0..1`.
    Rgba8Unorm,
    /// 8-bit per channel, sRGB-encoded storage — the stored byte is sRGB; the
    /// reported `rgba` is the linear value (the frontend toggle can show either).
    Rgba8UnormSrgb,
    /// 16-bit float per channel — values may exceed `1.0` (HDR/extended range).
    Rgba16Float,
}

/// The result of inspecting one preview pixel (#61).
///
/// Always returned (the inspector never errors on an out-of-bounds hover): when the
/// pane coordinate falls in a letterbox bar, `inside` is `false` and `rgba` is the
/// bar color (black) — the frontend reports "outside" rather than a bogus sample.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PixelSample {
    /// Whether the pane coordinate maps to a real content pixel (`true`) or landed
    /// in a letterbox/pillarbox bar outside the content rect (`false`).
    pub inside: bool,
    /// The SIMULATED-VIEWPORT pixel X the pane coordinate maps to (content-rect
    /// space; `0` when `inside` is false). NOT a raw pane pixel.
    pub x: u32,
    /// The SIMULATED-VIEWPORT pixel Y the pane coordinate maps to.
    pub y: u32,
    /// The simulated-viewport width the coordinate is reported against (the §9
    /// content-rect width — the resolution the final image actually occupies).
    pub viewport_width: u32,
    /// The simulated-viewport height the coordinate is reported against.
    pub viewport_height: u32,
    /// The pixel's RGBA **as stored in the offscreen target**, normalized to `0..1`
    /// (a byte/255 for the current `Rgba8Unorm` target). The pre-downsample value;
    /// the frontend applies any sRGB/linear and 0-255/0-1 display conversion.
    pub rgba: [f32; 4],
    /// The offscreen target's pixel format (today always `Rgba8Unorm`).
    pub format: PixelFormat,
}

impl PixelSample {
    /// An "outside" sample (the pane coordinate hit a letterbox bar): no content
    /// pixel, black, reported against the given simulated-viewport size.
    pub fn outside(viewport_width: u32, viewport_height: u32) -> Self {
        Self {
            inside: false,
            x: 0,
            y: 0,
            viewport_width,
            viewport_height,
            rgba: [0.0, 0.0, 0.0, 0.0],
            format: PixelFormat::Rgba8Unorm,
        }
    }
}
