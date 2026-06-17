//! Builtin + parameter uniform computation for a single preview pass
//! (Architecture §D). This is the Phase 1 slice: just enough of RetroArch's
//! builtin-semantics set for a one-pass curvature/warp shader — `MVP`, the
//! `*Size` family, and `FrameCount` — plus the `#pragma parameter` defaults
//! packed into their own UBO. Full semantic coverage and live slider updates are
//! Phase 2 (Specification §4).
//!
//! ## Layout convention
//!
//! These structs/functions assume the canonical Phase 1 UBO layout that the
//! fixture shaders declare. Real RetroArch shaders declare members in arbitrary
//! order and the offsets are discovered by reflecting the SPIR-V; doing that is a
//! Phase 2 import concern. Here both sides agree on a fixed std140 layout:
//!
//! ```glsl
//! layout(std140, set = 0, binding = 0) uniform UBO {
//!     mat4 MVP;          // offset 0
//!     vec4 SourceSize;   // offset 64
//!     vec4 OriginalSize; // offset 80
//!     vec4 OutputSize;   // offset 96
//!     uint FrameCount;   // offset 112
//! } global;
//! layout(std140, set = 0, binding = 3) uniform Params {
//!     float P0; float P1; ...   // in #pragma parameter declaration order
//! } params;
//! ```
//!
//! Each `*Size` vec4 is `[w, h, 1/w, 1/h]` — the RetroArch convention that lets a
//! shader fetch both a dimension and its reciprocal without a divide.

use slang_compile::Parameter;

/// The builtin uniforms for one pass, laid out to match the canonical std140 UBO
/// block (see the module docs). `#[repr(C)]` + field order/padding make the byte
/// image bindable directly via `bytemuck`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BuiltinUniforms {
    /// Model-view-projection matrix, column-major (offset 0).
    pub mvp: [f32; 16],
    /// `Source` texture size `[w, h, 1/w, 1/h]` (offset 64).
    pub source_size: [f32; 4],
    /// `Original` (pass-0 input) size; equals `source_size` in a one-pass slice
    /// (offset 80).
    pub original_size: [f32; 4],
    /// Render-target / simulated-viewport size (offset 96).
    pub output_size: [f32; 4],
    /// Frames rendered so far, for animated shaders (offset 112).
    pub frame_count: u32,
    /// Tail padding so the struct is a multiple of 16 bytes, as std140 requires
    /// for a UBO block. Always zero.
    pub pad: [u32; 3],
}

impl BuiltinUniforms {
    /// Compute the builtin set from the source-image and output (viewport) sizes
    /// and the current frame counter. `original` is the pass-0 input — in a
    /// one-pass slice it is the same image as `source`. Convenience wrapper over
    /// [`BuiltinUniforms::new_full`] for the single-pass case where
    /// `Source == Original`.
    pub fn new(source: (u32, u32), output: (u32, u32), frame_count: u32) -> Self {
        Self::new_full(source, source, output, frame_count)
    }

    /// Compute the builtin set for one pass of a multi-pass chain, where the
    /// pass's `Source` (its input), the chain's `Original` (the pass-0 input),
    /// and the pass's `Output` (its render target) can all differ (§2/§6). Pass 0
    /// has `source == original`; later passes' `source` is the previous FBO size.
    pub fn new_full(
        source: (u32, u32),
        original: (u32, u32),
        output: (u32, u32),
        frame_count: u32,
    ) -> Self {
        Self {
            mvp: ortho_mvp(),
            source_size: size_vec(source.0, source.1),
            original_size: size_vec(original.0, original.1),
            output_size: size_vec(output.0, output.1),
            frame_count,
            pad: [0; 3],
        }
    }

    /// The struct as raw bytes ready to upload into the builtin UBO.
    pub fn as_bytes(&self) -> &[u8] {
        bytemuck::bytes_of(self)
    }
}

/// A RetroArch `*Size` vector: `[w, h, 1/w, 1/h]`. Dimensions are clamped to at
/// least 1 so the reciprocals are always finite.
pub fn size_vec(width: u32, height: u32) -> [f32; 4] {
    let w = width.max(1) as f32;
    let h = height.max(1) as f32;
    [w, h, 1.0 / w, 1.0 / h]
}

/// The fullscreen-pass MVP: an orthographic projection mapping the unit-square
/// quad (positions in `[0,1]`) to clip space `[-1,1]`. This is RetroArch's
/// standard pass matrix and, like RetroArch's, it does not depend on the
/// viewport size — the viewport governs rasterization, not this transform.
/// Returned column-major to match GLSL/SPIR-V `mat4` storage.
pub fn ortho_mvp() -> [f32; 16] {
    [
        2.0, 0.0, 0.0, 0.0, // column 0
        0.0, 2.0, 0.0, 0.0, // column 1
        0.0, 0.0, 1.0, 0.0, // column 2
        -1.0, -1.0, 0.0, 1.0, // column 3 (translation)
    ]
}

/// Pack the parameter defaults into the std140 parameter-UBO byte image: each
/// `#pragma parameter` default as a consecutive `f32` (4-byte std140 scalar
/// packing), in declaration order, then padded up to a multiple of 16 bytes
/// (never empty — a bound UBO needs at least one vec4 of storage even when the
/// shader declares no parameters). Live updates are Phase 2; this writes the
/// reflected defaults only.
pub fn pack_parameters(params: &[Parameter]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(params.len() * 4);
    for p in params {
        bytes.extend_from_slice(&p.default.to_le_bytes());
    }
    let padded = bytes.len().div_ceil(16).max(1) * 16;
    bytes.resize(padded, 0);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::offset_of;

    fn param(name: &str, default: f32) -> Parameter {
        Parameter {
            name: name.to_string(),
            label: name.to_string(),
            default,
            min: 0.0,
            max: 1.0,
            step: 0.0,
        }
    }

    #[test]
    fn size_vec_is_w_h_and_reciprocals() {
        assert_eq!(size_vec(4, 2), [4.0, 2.0, 0.25, 0.5]);
        // Zero dimensions clamp to 1 so the reciprocal stays finite.
        assert_eq!(size_vec(0, 0), [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn ubo_field_offsets_match_std140() {
        // The byte image must match the shader's declared block exactly.
        assert_eq!(offset_of!(BuiltinUniforms, mvp), 0);
        assert_eq!(offset_of!(BuiltinUniforms, source_size), 64);
        assert_eq!(offset_of!(BuiltinUniforms, original_size), 80);
        assert_eq!(offset_of!(BuiltinUniforms, output_size), 96);
        assert_eq!(offset_of!(BuiltinUniforms, frame_count), 112);
        // std140 rounds the block up to a multiple of 16.
        assert_eq!(std::mem::size_of::<BuiltinUniforms>(), 128);
    }

    #[test]
    fn builtins_compute_sizes_and_carry_frame_count() {
        let u = BuiltinUniforms::new((320, 240), (640, 480), 7);
        assert_eq!(u.source_size, [320.0, 240.0, 1.0 / 320.0, 1.0 / 240.0]);
        assert_eq!(u.original_size, u.source_size);
        assert_eq!(u.output_size, [640.0, 480.0, 1.0 / 640.0, 1.0 / 480.0]);
        assert_eq!(u.frame_count, 7);
        assert_eq!(u.pad, [0; 3]);
    }

    #[test]
    fn mvp_maps_unit_quad_to_clip_space() {
        let m = ortho_mvp();
        // Column-major mat4 * vec4(x, y, 0, 1).
        let apply = |x: f32, y: f32| {
            let p = [x, y, 0.0, 1.0];
            let mut out = [0.0f32; 4];
            for (row, slot) in out.iter_mut().enumerate() {
                *slot = (0..4).map(|col| m[col * 4 + row] * p[col]).sum();
            }
            (out[0], out[1])
        };
        // The unit square's corners map onto the clip-space corners.
        assert_eq!(apply(0.0, 0.0), (-1.0, -1.0));
        assert_eq!(apply(1.0, 1.0), (1.0, 1.0));
        assert_eq!(apply(0.5, 0.5), (0.0, 0.0));
    }

    #[test]
    fn parameters_pack_consecutively_and_pad_to_16() {
        let bytes = pack_parameters(&[param("A", 0.5), param("B", 0.25)]);
        assert_eq!(bytes.len(), 16); // 2 floats -> padded up to 16
        assert_eq!(&bytes[0..4], &0.5f32.to_le_bytes());
        assert_eq!(&bytes[4..8], &0.25f32.to_le_bytes());
        assert_eq!(&bytes[8..16], &[0u8; 8]); // padding is zero

        // Five floats spill into a second 16-byte stride.
        let many: Vec<_> = (0..5).map(|i| param("P", i as f32)).collect();
        assert_eq!(pack_parameters(&many).len(), 32);
    }

    #[test]
    fn no_parameters_still_yields_one_vec4() {
        // A bound UBO needs storage even when the shader declares no parameters.
        assert_eq!(pack_parameters(&[]), vec![0u8; 16]);
    }
}
