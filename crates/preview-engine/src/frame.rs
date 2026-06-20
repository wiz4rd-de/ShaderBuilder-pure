//! Binary preview-frame format — the transport contract between the preview
//! engine and the webview `<canvas>` (Architecture §E/§F).
//!
//! Each frame is a little-endian **24-byte header** followed by tightly packed
//! `RGBA8` pixels (`width * height * 4` bytes):
//!
//! | offset | size | field        | notes                          |
//! |--------|------|--------------|--------------------------------|
//! | 0      | 4    | magic        | ASCII `"SBF1"`                 |
//! | 4      | 2    | version      | `u16`, currently 1            |
//! | 6      | 1    | pixel format | `0` = RGBA8                    |
//! | 7      | 1    | reserved     | `0`                           |
//! | 8      | 4    | width        | `u32` pixels                  |
//! | 12     | 4    | height       | `u32` pixels                  |
//! | 16     | 8    | frame index  | `u64`, increments per frame   |
//!
//! The exact same layout is parsed on the frontend in `web/src/preview/frame.ts`.

/// Magic bytes at the start of every frame: ASCII `"SBF1"`.
pub const FRAME_MAGIC: [u8; 4] = *b"SBF1";
/// Current frame-format version.
pub const FRAME_VERSION: u16 = 1;
/// Size of the fixed binary header in bytes.
pub const FRAME_HEADER_LEN: usize = 24;
/// Pixel-format tag for tightly packed 8-bit RGBA.
pub const PIXEL_FORMAT_RGBA8: u8 = 0;

/// Header describing one streamed frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Monotonically increasing frame index.
    pub frame_index: u64,
    /// Pixel format tag; see [`PIXEL_FORMAT_RGBA8`].
    pub format: u8,
}

impl FrameHeader {
    /// A header for an RGBA8 frame.
    pub fn rgba8(width: u32, height: u32, frame_index: u64) -> Self {
        Self {
            width,
            height,
            frame_index,
            format: PIXEL_FORMAT_RGBA8,
        }
    }

    /// Number of pixel bytes that must follow this header.
    pub fn payload_len(&self) -> usize {
        self.width as usize * self.height as usize * 4
    }

    /// Append the 24-byte header to `buf`.
    pub fn write_to(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&FRAME_MAGIC);
        buf.extend_from_slice(&FRAME_VERSION.to_le_bytes());
        buf.push(self.format);
        buf.push(0); // reserved
        buf.extend_from_slice(&self.width.to_le_bytes());
        buf.extend_from_slice(&self.height.to_le_bytes());
        buf.extend_from_slice(&self.frame_index.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_serializes_to_24_le_bytes() {
        let mut buf = Vec::new();
        FrameHeader::rgba8(320, 240, 7).write_to(&mut buf);
        assert_eq!(buf.len(), FRAME_HEADER_LEN);
        assert_eq!(&buf[0..4], b"SBF1");
        assert_eq!(u16::from_le_bytes([buf[4], buf[5]]), FRAME_VERSION);
        assert_eq!(buf[6], PIXEL_FORMAT_RGBA8);
        assert_eq!(u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]), 320);
        assert_eq!(
            u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]),
            240
        );
        assert_eq!(buf[16], 7);
    }

    #[test]
    fn payload_len_matches_rgba8() {
        assert_eq!(FrameHeader::rgba8(4, 3, 0).payload_len(), 48);
    }
}
