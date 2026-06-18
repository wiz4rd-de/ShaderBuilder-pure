//! `codegen-slang` — emits Vulkan-GLSL slang source from the lowered IR
//! (UBO `layout(set/binding)`, `#pragma stage`, semantics). The primary backend;
//! its output is what the preview engine compiles and renders (Architecture §C).
//!
//! The entry point is [`emit_pass`]: it consumes a lowered, type-checked pass
//! ([`ir::LoweredIr`] + its [`ir::PassManifest`], #41) plus [`EmitOptions`] (the
//! pass alias/format and the full `#pragma parameter` declarations) and returns a
//! complete `.slang` source string in RetroArch's Vulkan-GLSL conventions. The
//! emitted source is designed to compile through `slang_compile::compile_slang`
//! with no errors (the #42 acceptance bar) and to render identically to a
//! hand-written equivalent through the proven engine (#44).
//!
//! See [`emit`] for the emitted shape and the binding/semantic conventions.

pub mod emit;

pub use emit::{emit_pass, texture_slang_name, EmitOptions, DEFAULT_FORMAT};

/// Crate identity marker. See [`core_model::NAME`].
pub const NAME: &str = "codegen-slang";

#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {
        assert_eq!(super::NAME, "codegen-slang");
        // The `codegen-slang` → `ir` dependency edge is real and exercised.
        assert_eq!(ir::NAME, "ir");
    }
}
