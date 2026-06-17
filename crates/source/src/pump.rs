//! The **frame pump** (#31, Architecture §D): the producer the render thread
//! steps once per *source frame* to drive `Original`/`Source`, the history ring,
//! and (on seek) feedback reset. A pump exposes the *current* [`Frame`] plus
//! `advance`/`seek`/`position`/`len`, so the render thread can pace advances
//! against its own clock without knowing whether the content is a still image, a
//! procedural test pattern, or a decoded PNG sequence.
//!
//! This is deliberately **named `FramePump`, not `FrameSource`** — the latter is
//! `preview_engine::FrameSource`, a *different* concept (the render→transport
//! seam). The pump sits *inside* a `RenderSource` and feeds the renderer.
//!
//! ## Temporal model (`docs/retroarch-slang-runtime.md` §5/§10)
//! Advancing the pump to a *new* source frame is exactly the trigger the runtime
//! calls `push_history` for (§5/§10 step 5): the render thread responds with
//! `Renderer::advance_source` (rotate history once + set the new `Original`). A
//! `seek` (or the first frame) is a reload — `Renderer::set_source` (reset the
//! history ring), and on seek also `Renderer::reset_feedback`. None of this
//! touches `FrameCount`/feedback-swap, which stay per *rendered* frame (§10): the
//! pump's fps is the *content* clock, the render loop's ~60 fps is the *animation*
//! clock. **No ffmpeg**: PNG sequences decode in-core via the `image` crate.

use std::path::Path;

use crate::{load_image, Frame, SourceError};

/// A producer of source frames for the preview pipeline (#31).
///
/// The render thread holds one of these behind a `Box<dyn FramePump + Send>` and
/// steps it once per source frame (`advance`) or to a chosen index (`seek`),
/// reading [`FramePump::current`] for the `Original`/`Source` to upload. A still
/// image or a static test pattern reports `len() == 1` and ignores advance/seek;
/// an animated pattern or a PNG sequence reports `len() > 1` and loops.
///
/// `len()` is guaranteed `>= 1` for every implementation here, so there is no
/// empty pump (hence no `is_empty`): `current` always returns a real frame.
// `len` is always `>= 1` (an empty pump can't exist — `current` must return a
// real frame), so an `is_empty` would be a dead `false`. Suppress the lint.
#[allow(clippy::len_without_is_empty)]
pub trait FramePump {
    /// The frame currently presented as `Original`/`Source`.
    fn current(&self) -> &Frame;

    /// Step to the next frame, **looping** back to 0 at the end (§5 loop wrap).
    /// A no-op for a single-frame source (still image / static pattern).
    fn advance(&mut self);

    /// Jump to frame `index`, taken **modulo `len()`** so any index is valid
    /// (and loop-relative seeks Just Work). A no-op-equivalent for a 1-frame
    /// source (every index maps to frame 0).
    fn seek(&mut self, index: usize);

    /// The current frame index in `0..len()`.
    fn position(&self) -> usize;

    /// The number of frames this pump cycles through (`>= 1`). `1` for a still
    /// image or a static test pattern.
    fn len(&self) -> usize;
}

/// A single still image presented forever (#31): `len() == 1`, `advance`/`seek`
/// are no-ops, `current` returns the one frame. Wraps a one-shot `SetSource`
/// frame and any decoded still PNG/JPEG.
pub struct StillImage(Frame);

impl StillImage {
    /// Wrap a decoded frame as a one-frame pump.
    pub fn new(frame: Frame) -> Self {
        Self(frame)
    }
}

impl FramePump for StillImage {
    fn current(&self) -> &Frame {
        &self.0
    }
    fn advance(&mut self) {}
    fn seek(&mut self, _index: usize) {}
    fn position(&self) -> usize {
        0
    }
    fn len(&self) -> usize {
        1
    }
}

/// The built-in procedural test patterns (#31). The static ones report a single
/// frame; [`TestPattern::MotionSweep`] is animated (a moving bar) so it has a
/// fixed multi-frame cycle and history tests can observe motion across advances.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestPattern {
    /// SMPTE-style vertical color bars (static).
    SmpteBars,
    /// A high-contrast checkerboard (static) — the legacy [`crate::test_pattern`].
    Checkerboard,
    /// A two-axis RGB gradient (static).
    Gradient,
    /// A vertical bar that sweeps across the frame (animated, [`MOTION_SWEEP_FRAMES`]
    /// long), so successive `advance`s yield visibly different frames.
    MotionSweep,
}

/// The fixed cycle length of [`TestPattern::MotionSweep`] (#31): a small, round
/// number so history/loop tests are quick and the sweep wraps predictably.
pub const MOTION_SWEEP_FRAMES: usize = 60;

impl TestPattern {
    /// The number of distinct frames in this pattern's cycle: 1 for the static
    /// patterns, [`MOTION_SWEEP_FRAMES`] for the animated sweep.
    // A pattern always has `>= 1` frame, so there is no meaningful `is_empty`.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        match self {
            TestPattern::MotionSweep => MOTION_SWEEP_FRAMES,
            _ => 1,
        }
    }

    /// Render frame `index` of this pattern at `width × height` as an RGBA8
    /// [`Frame`]. `index` is taken modulo the pattern's cycle length; static
    /// patterns ignore it entirely.
    pub fn render(&self, width: u32, height: u32, index: usize) -> Frame {
        let width = width.max(1);
        let height = height.max(1);
        match self {
            TestPattern::SmpteBars => smpte_bars(width, height),
            TestPattern::Checkerboard => crate::test_pattern(width, height),
            TestPattern::Gradient => gradient(width, height),
            TestPattern::MotionSweep => motion_sweep(width, height, index % MOTION_SWEEP_FRAMES),
        }
    }
}

/// Classic SMPTE-style vertical color bars: seven equal columns of
/// white/yellow/cyan/green/magenta/red/blue across the width.
fn smpte_bars(width: u32, height: u32) -> Frame {
    const BARS: [[u8; 3]; 7] = [
        [192, 192, 192], // white (75%)
        [192, 192, 0],   // yellow
        [0, 192, 192],   // cyan
        [0, 192, 0],     // green
        [192, 0, 192],   // magenta
        [192, 0, 0],     // red
        [0, 0, 192],     // blue
    ];
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for _y in 0..height {
        for x in 0..width {
            // Map the column into one of seven bars by horizontal position.
            let bar = (x as usize * BARS.len() / width as usize).min(BARS.len() - 1);
            let [r, g, b] = BARS[bar];
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
    }
    Frame::new(width, height, rgba)
}

/// A two-axis RGB gradient: red ramps left→right, green ramps top→bottom, blue
/// fixed mid — a smooth field that makes warping/filtering obvious.
fn gradient(width: u32, height: u32) -> Frame {
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        let g = (y * 255 / height) as u8;
        for x in 0..width {
            let r = (x * 255 / width) as u8;
            rgba.extend_from_slice(&[r, g, 128, 255]);
        }
    }
    Frame::new(width, height, rgba)
}

/// A vertical white sweep bar over a dark field, positioned by `phase` within the
/// [`MOTION_SWEEP_FRAMES`] cycle: the bar's left edge marches across the width so
/// frame N and frame N+1 differ wherever the bar moves (the basis for the
/// "MotionSweep frames differ" history test).
fn motion_sweep(width: u32, height: u32, phase: usize) -> Frame {
    // The bar is ~1/8 of the width and its position is phase/cycle across [0,width).
    let bar_w = (width / 8).max(1);
    let sweep_x = (phase as u32 * width / MOTION_SWEEP_FRAMES as u32) % width;
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for _y in 0..height {
        for x in 0..width {
            // Distance from the bar's left edge, wrapping at the width so the bar
            // re-enters from the left edge as it leaves the right.
            let within = x.wrapping_sub(sweep_x) % width < bar_w;
            let px = if within {
                [255, 255, 255, 255]
            } else {
                [16, 16, 24, 255]
            };
            rgba.extend_from_slice(&px);
        }
    }
    Frame::new(width, height, rgba)
}

/// A pump rendering a [`TestPattern`] at a fixed size (#31). Static patterns are
/// a 1-frame pump (advance/seek no-ops); [`TestPattern::MotionSweep`] cycles
/// through [`MOTION_SWEEP_FRAMES`] frames, re-rendering the current phase lazily
/// on each `advance`/`seek`.
pub struct TestPatternPump {
    pattern: TestPattern,
    width: u32,
    height: u32,
    position: usize,
    len: usize,
    current: Frame,
}

impl TestPatternPump {
    /// Build a pump for `pattern` at `width × height` (clamped to at least 1×1),
    /// positioned at frame 0.
    pub fn new(pattern: TestPattern, width: u32, height: u32) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let len = pattern.len();
        let current = pattern.render(width, height, 0);
        Self {
            pattern,
            width,
            height,
            position: 0,
            len,
            current,
        }
    }

    /// Re-render the current phase into `self.current` (after a position change).
    fn refresh(&mut self) {
        self.current = self.pattern.render(self.width, self.height, self.position);
    }
}

impl FramePump for TestPatternPump {
    fn current(&self) -> &Frame {
        &self.current
    }

    fn advance(&mut self) {
        if self.len <= 1 {
            return; // static pattern: nothing to advance
        }
        self.position = (self.position + 1) % self.len;
        self.refresh();
    }

    fn seek(&mut self, index: usize) {
        if self.len <= 1 {
            return;
        }
        let pos = index % self.len;
        if pos != self.position {
            self.position = pos;
            self.refresh();
        }
    }

    fn position(&self) -> usize {
        self.position
    }

    fn len(&self) -> usize {
        self.len
    }
}

/// A numbered **PNG-sequence** player (#31): all frames are decoded up front into
/// a `Vec<Frame>` (acceptable for v1 — a sequence is finite and the decode stays
/// off the render loop, in the app command). `advance` increments modulo `len`
/// (loops); `seek` jumps to `index % len`.
pub struct PngSequencePump {
    frames: Vec<Frame>,
    position: usize,
}

impl PngSequencePump {
    /// Load a numbered directory of PNGs (#31). Files whose stem ends in a run of
    /// digits (`frame_0001.png`, `0001.png`, `frame_2.png`, …) are collected and
    /// sorted by the **parsed trailing number** — not lexically — so `frame_2`
    /// precedes `frame_10`. Zero-padded and non-padded names mix freely. Each
    /// match is decoded via the same `image`-crate path as [`crate::load_image`].
    ///
    /// Returns [`SourceError::NoFrames`] if the directory contains no numbered
    /// PNG. All frames are decoded eagerly and held in memory.
    pub fn load(dir: &Path) -> Result<Self, SourceError> {
        let mut numbered: Vec<(u64, std::path::PathBuf)> = Vec::new();
        for entry in std::fs::read_dir(dir).map_err(SourceError::Io)? {
            let entry = entry.map_err(SourceError::Io)?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            // Only `.png` files, matched case-insensitively.
            let is_png = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("png"));
            if !is_png {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if let Some(n) = trailing_number(stem) {
                numbered.push((n, path));
            }
        }
        if numbered.is_empty() {
            return Err(SourceError::NoFrames);
        }
        // Sort by the parsed number (numeric, not lexical), tie-breaking on the
        // path so the order is deterministic for equal numbers.
        numbered.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

        let mut frames = Vec::with_capacity(numbered.len());
        for (_, path) in numbered {
            frames.push(load_image(&path)?);
        }
        Ok(Self {
            frames,
            position: 0,
        })
    }

    /// Build a sequence pump from already-decoded frames (#31). The app decodes
    /// PNGs off the render loop and ships the `Vec<Frame>` over IPC; the render
    /// thread builds the pump from it. `frames` must be non-empty.
    ///
    /// # Panics
    /// If `frames` is empty (a pump must always have a current frame).
    pub fn from_frames(frames: Vec<Frame>) -> Self {
        assert!(!frames.is_empty(), "a PNG-sequence pump needs >= 1 frame");
        Self {
            frames,
            position: 0,
        }
    }

    /// Consume the pump, yielding its decoded frames in sequence order (#31). The
    /// app uses this to ship the `Vec<Frame>` over IPC after a [`Self::load`]
    /// (file IO stays in the command; the render thread builds the pump from the
    /// frames). Always non-empty.
    pub fn into_frames(self) -> Vec<Frame> {
        self.frames
    }
}

impl FramePump for PngSequencePump {
    fn current(&self) -> &Frame {
        &self.frames[self.position]
    }

    fn advance(&mut self) {
        self.position = (self.position + 1) % self.frames.len();
    }

    fn seek(&mut self, index: usize) {
        self.position = index % self.frames.len();
    }

    fn position(&self) -> usize {
        self.position
    }

    fn len(&self) -> usize {
        self.frames.len()
    }
}

/// Parse the trailing run of ASCII digits of `stem` into a number, if any
/// (`frame_0007` → 7, `0010` → 10, `frame_2` → 2). Returns `None` when the stem
/// ends in a non-digit (no frame number to sort by). Saturates on overflow.
fn trailing_number(stem: &str) -> Option<u64> {
    let digits: String = stem
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if digits.is_empty() {
        return None;
    }
    Some(digits.parse::<u64>().unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Write a solid-color PNG of `w × h` to `path` (for sequence fixtures).
    fn write_png(path: &Path, w: u32, h: u32, rgba: [u8; 4]) {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba(rgba));
        let mut bytes = std::io::Cursor::new(Vec::new());
        img.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        std::fs::File::create(path)
            .unwrap()
            .write_all(bytes.get_ref())
            .unwrap();
    }

    // ---- StillImage ----

    #[test]
    fn still_image_is_one_frame_and_ignores_advance_seek() {
        let frame = Frame::new(
            2,
            2,
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
        );
        let mut pump = StillImage::new(frame.clone());
        assert_eq!(pump.len(), 1);
        assert_eq!(pump.position(), 0);
        assert_eq!(pump.current(), &frame);
        pump.advance();
        pump.seek(7);
        assert_eq!(pump.position(), 0, "still image never moves");
        assert_eq!(pump.current(), &frame);
    }

    // ---- Test patterns ----

    #[test]
    fn static_patterns_are_valid_rgba_and_single_frame() {
        for pattern in [
            TestPattern::SmpteBars,
            TestPattern::Checkerboard,
            TestPattern::Gradient,
        ] {
            let mut pump = TestPatternPump::new(pattern, 64, 48);
            assert_eq!(pump.len(), 1, "{pattern:?} is static");
            let frame = pump.current();
            assert_eq!((frame.width, frame.height), (64, 48), "{pattern:?} size");
            assert_eq!(frame.rgba.len(), 64 * 48 * 4, "{pattern:?} RGBA length");
            // advance/seek are no-ops on a static pattern.
            let before = frame.clone();
            pump.advance();
            pump.seek(5);
            assert_eq!(pump.position(), 0);
            assert_eq!(pump.current(), &before, "{pattern:?} static across advance");
        }
    }

    #[test]
    fn smpte_bars_have_multiple_distinct_columns() {
        let pump = TestPatternPump::new(TestPattern::SmpteBars, 70, 4);
        let f = pump.current();
        // The far-left column (white-ish) differs from the far-right (blue).
        let left = &f.rgba[0..4];
        let right_x = (f.width - 1) as usize;
        let right = &f.rgba[right_x * 4..right_x * 4 + 4];
        assert_ne!(left, right, "SMPTE bars must vary across the width");
    }

    #[test]
    fn motion_sweep_frames_differ_and_loop() {
        let mut pump = TestPatternPump::new(TestPattern::MotionSweep, 64, 16);
        assert_eq!(pump.len(), MOTION_SWEEP_FRAMES);
        let f0 = pump.current().clone();
        assert_eq!(f0.rgba.len(), 64 * 16 * 4);

        pump.advance();
        assert_eq!(pump.position(), 1);
        let f1 = pump.current().clone();
        assert_ne!(
            f0.rgba, f1.rgba,
            "MotionSweep frame 1 must differ from frame 0 (the bar moved)"
        );

        // Advancing through the full cycle returns to frame 0 (loop wrap).
        for _ in 1..MOTION_SWEEP_FRAMES {
            pump.advance();
        }
        assert_eq!(pump.position(), 0, "MotionSweep loops after its cycle");
        assert_eq!(pump.current().rgba, f0.rgba, "looped frame matches frame 0");
    }

    #[test]
    fn motion_sweep_seek_jumps_and_wraps() {
        let mut pump = TestPatternPump::new(TestPattern::MotionSweep, 64, 16);
        pump.seek(MOTION_SWEEP_FRAMES + 3); // modulo -> 3
        assert_eq!(pump.position(), 3);
        let at3 = pump.current().clone();
        pump.seek(3);
        assert_eq!(
            pump.current().rgba,
            at3.rgba,
            "seek is deterministic by index"
        );
    }

    // ---- PNG sequence ----

    #[test]
    fn png_sequence_loads_advances_loops_and_seeks() {
        let dir = tempfile::tempdir().unwrap();
        // Write three frames OUT OF lexical-vs-numeric order on purpose:
        // frame_2 must sort BEFORE frame_10 (numeric sort, not "10" < "2").
        write_png(&dir.path().join("frame_2.png"), 4, 3, [10, 0, 0, 255]);
        write_png(&dir.path().join("frame_10.png"), 4, 3, [20, 0, 0, 255]);
        write_png(&dir.path().join("frame_1.png"), 4, 3, [30, 0, 0, 255]);
        // A non-numbered PNG and a non-PNG are ignored.
        write_png(&dir.path().join("ignore.png"), 4, 3, [99, 0, 0, 255]);
        std::fs::write(dir.path().join("notes.txt"), b"hi").unwrap();

        let mut pump = PngSequencePump::load(dir.path()).expect("load sequence");
        assert_eq!(pump.len(), 3, "three numbered PNGs collected");
        assert_eq!((pump.current().width, pump.current().height), (4, 3));

        // Numeric order: frame_1 (R30), frame_2 (R10), frame_10 (R20).
        assert_eq!(pump.current().rgba[0], 30, "position 0 is frame_1");
        pump.advance();
        assert_eq!(pump.current().rgba[0], 10, "position 1 is frame_2");
        pump.advance();
        assert_eq!(pump.current().rgba[0], 20, "position 2 is frame_10");

        // Loop wrap: advance from the last frame returns to frame 0.
        pump.advance();
        assert_eq!(pump.position(), 0);
        assert_eq!(pump.current().rgba[0], 30, "advance loops to frame_1");

        // Seek jumps + wraps modulo len.
        pump.seek(2);
        assert_eq!(pump.current().rgba[0], 20, "seek(2) -> frame_10");
        pump.seek(3); // modulo 3 -> 0
        assert_eq!(pump.current().rgba[0], 30, "seek wraps modulo len");
    }

    #[test]
    fn png_sequence_empty_dir_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("readme.txt"), b"no pngs here").unwrap();
        match PngSequencePump::load(dir.path()) {
            Err(SourceError::NoFrames) => {}
            Err(other) => panic!("expected NoFrames, got {other:?}"),
            Ok(_) => panic!("expected NoFrames error for a dir with no numbered PNGs"),
        }
    }

    #[test]
    fn png_sequence_from_decoded_frames() {
        // The app ships decoded frames over IPC; build the pump from them.
        let frames = vec![
            Frame::new(1, 1, vec![1, 0, 0, 255]),
            Frame::new(1, 1, vec![2, 0, 0, 255]),
        ];
        let mut pump = PngSequencePump::from_frames(frames);
        assert_eq!(pump.len(), 2);
        assert_eq!(pump.current().rgba[0], 1);
        pump.advance();
        assert_eq!(pump.current().rgba[0], 2);
        pump.advance();
        assert_eq!(pump.current().rgba[0], 1, "loops");
    }

    #[test]
    fn trailing_number_parses_padded_and_unpadded() {
        assert_eq!(trailing_number("frame_0007"), Some(7));
        assert_eq!(trailing_number("0010"), Some(10));
        assert_eq!(trailing_number("frame_2"), Some(2));
        assert_eq!(trailing_number("noframe"), None);
        assert_eq!(trailing_number("img12_3"), Some(3));
    }
}
