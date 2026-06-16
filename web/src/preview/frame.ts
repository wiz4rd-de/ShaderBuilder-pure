// Parser for the binary preview-frame format streamed from Rust.
//
// Must match the layout written in crates/preview-engine/src/frame.rs:
// a little-endian 24-byte header followed by `width * height * 4` RGBA8 bytes.

export const FRAME_HEADER_LEN = 24;
export const FRAME_VERSION = 1;
export const PIXEL_FORMAT_RGBA8 = 0;

export interface PreviewFrame {
  width: number;
  height: number;
  frameIndex: number;
  format: number;
  /** RGBA8 pixels, ready to hand to `ImageData`. */
  pixels: Uint8ClampedArray<ArrayBuffer>;
}

/** Normalize whatever a Tauri Channel hands us into a standalone ArrayBuffer. */
export function toArrayBuffer(message: unknown): ArrayBuffer {
  if (message instanceof ArrayBuffer) {
    return message;
  }
  let bytes: Uint8Array;
  if (message instanceof Uint8Array) {
    bytes = message;
  } else if (ArrayBuffer.isView(message)) {
    const view = message as ArrayBufferView;
    bytes = new Uint8Array(view.buffer as ArrayBuffer, view.byteOffset, view.byteLength);
  } else if (Array.isArray(message)) {
    bytes = new Uint8Array(message as number[]);
  } else {
    throw new Error("unexpected preview channel message type");
  }
  // Copy into a fresh, plain ArrayBuffer (never a SharedArrayBuffer / sub-view).
  const out = new ArrayBuffer(bytes.byteLength);
  new Uint8Array(out).set(bytes);
  return out;
}

/** Parse a raw frame buffer (header + pixels). Throws on a malformed frame. */
export function parseFrame(buffer: ArrayBuffer): PreviewFrame {
  if (buffer.byteLength < FRAME_HEADER_LEN) {
    throw new Error("preview frame shorter than header");
  }
  const view = new DataView(buffer);
  if (
    view.getUint8(0) !== 0x53 || // S
    view.getUint8(1) !== 0x42 || // B
    view.getUint8(2) !== 0x46 || // F
    view.getUint8(3) !== 0x31 //    1
  ) {
    throw new Error("bad preview frame magic");
  }
  const version = view.getUint16(4, true);
  if (version !== FRAME_VERSION) {
    throw new Error(`unsupported preview frame version ${version}`);
  }
  const format = view.getUint8(6);
  const width = view.getUint32(8, true);
  const height = view.getUint32(12, true);
  // frame index is u64 LE; reassemble from two u32 halves.
  const indexLow = view.getUint32(16, true);
  const indexHigh = view.getUint32(20, true);
  const frameIndex = indexHigh * 0x1_0000_0000 + indexLow;

  const expected = width * height * 4;
  if (buffer.byteLength < FRAME_HEADER_LEN + expected) {
    throw new Error("preview frame payload truncated");
  }
  // Copy the pixel region into its own buffer so it's a plain, standalone
  // Uint8ClampedArray<ArrayBuffer> (what ImageData requires).
  const pixels = new Uint8ClampedArray(expected);
  pixels.set(new Uint8Array(buffer, FRAME_HEADER_LEN, expected));
  return { width, height, frameIndex, format, pixels };
}
