//! `source` — the preview's frame pump. Phase 1 shipped the minimum the render
//! slice needed: a still-image loader and a built-in test pattern, both producing
//! a CPU [`Frame`] of RGBA8 pixels ready to upload to a wgpu texture. Phase 2
//! (#31, Architecture §D) adds the [`pump`] module: a [`pump::FramePump`]
//! abstraction with still-image, procedural test-pattern, and **in-core PNG-
//! sequence** producers (no ffmpeg — decoding goes through the `image` crate).
//! Video and richer sources remain a later, pluggable concern.

pub mod pump;

pub use pump::{FramePump, PngSequencePump, StillImage, TestPattern, TestPatternPump};

use std::path::Path;

/// Crate identity marker (kept from the Phase 0 scaffold so dependent crates'
/// smoke tests keep the dependency edge live).
pub const NAME: &str = "source";

/// A decoded source frame: tightly packed RGBA8, `rgba.len() == width*height*4`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Row-major RGBA8 pixels (4 bytes/pixel, no padding).
    pub rgba: Vec<u8>,
}

impl Frame {
    /// Construct a frame, validating the buffer length.
    ///
    /// # Panics
    /// If `rgba.len() != width * height * 4`.
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        assert_eq!(
            rgba.len(),
            width as usize * height as usize * 4,
            "RGBA buffer must be width*height*4 bytes"
        );
        Self {
            width,
            height,
            rgba,
        }
    }
}

/// Errors loading a source image or sequence.
#[derive(Debug)]
pub enum SourceError {
    /// The file could not be read.
    Io(std::io::Error),
    /// The image bytes could not be decoded.
    Decode(image::ImageError),
    /// A PNG-sequence directory contained no numbered PNG frames (#31).
    NoFrames,
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceError::Io(e) => write!(f, "could not read image: {e}"),
            SourceError::Decode(e) => write!(f, "could not decode image: {e}"),
            SourceError::NoFrames => write!(f, "no numbered PNG frames found in the directory"),
        }
    }
}

impl std::error::Error for SourceError {}

/// Load a still image (PNG/JPEG) from disk and decode it to an RGBA8 [`Frame`].
pub fn load_image(path: impl AsRef<Path>) -> Result<Frame, SourceError> {
    let bytes = std::fs::read(path.as_ref()).map_err(SourceError::Io)?;
    let decoded = image::load_from_memory(&bytes).map_err(SourceError::Decode)?;
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(Frame::new(width, height, rgba.into_raw()))
}

/// A built-in checkerboard test pattern — available without any filesystem
/// access so the render slice can always run. Bright, high-contrast cells make
/// warping/curvature obvious in the preview.
pub fn test_pattern(width: u32, height: u32) -> Frame {
    let width = width.max(1);
    let height = height.max(1);
    let cell = (width.min(height) / 8).max(1);
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            let checker = ((x / cell) + (y / cell)).is_multiple_of(2);
            // Tint by position so orientation is visible too.
            let r = (x * 255 / width) as u8;
            let g = (y * 255 / height) as u8;
            let px = if checker {
                [r, g, 32, 255]
            } else {
                [16, 16, 16, 255]
            };
            rgba.extend_from_slice(&px);
        }
    }
    Frame::new(width, height, rgba)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn smoke() {
        assert_eq!(NAME, "source");
        assert_eq!(core_model::NAME, "core-model");
    }

    #[test]
    fn test_pattern_is_valid_rgba() {
        let frame = test_pattern(64, 48);
        assert_eq!(frame.width, 64);
        assert_eq!(frame.height, 48);
        assert_eq!(frame.rgba.len(), 64 * 48 * 4);
    }

    #[test]
    fn loads_a_png_fixture() {
        // Encode a small fixture PNG, then decode it back through load_image.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture.png");
        let img = image::RgbaImage::from_pixel(5, 3, image::Rgba([10, 20, 30, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        img.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        std::fs::File::create(&path)
            .unwrap()
            .write_all(bytes.get_ref())
            .unwrap();

        let frame = load_image(&path).unwrap();
        assert_eq!((frame.width, frame.height), (5, 3));
        assert_eq!(frame.rgba.len(), 5 * 3 * 4);
        assert_eq!(&frame.rgba[0..4], &[10, 20, 30, 255]);
    }

    #[test]
    fn missing_file_is_an_error() {
        let err = load_image("/no/such/image.png").unwrap_err();
        assert!(matches!(err, SourceError::Io(_)));
    }
}
