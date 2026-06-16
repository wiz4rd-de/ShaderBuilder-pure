//! `codegen-slang` — emits Vulkan-GLSL slang source from the lowered IR
//! (UBO `layout(set/binding)`, `#pragma stage`, semantics). The primary backend;
//! its output is what the preview engine compiles and renders (Architecture §C).
//!
//! Phase 0: stub. The emitter lands in Phase 4 (#…).

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
