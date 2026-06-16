//! `codegen-glslp` — best-effort `IR → glslp` emitter for legacy export
//! (post-v1). Works off the same lowered IR as `codegen-slang`; custom
//! slang-dialect snippets are flagged when untranslatable (Architecture §C).
//!
//! Phase 0: stub. This is a post-v1 backlog item (#9); the crate exists now only
//! so the IR is designed against two backends from the start.

/// Crate identity marker. See [`core_model::NAME`].
pub const NAME: &str = "codegen-glslp";

#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {
        assert_eq!(super::NAME, "codegen-glslp");
        // The `codegen-glslp` → `ir` dependency edge is real and exercised.
        assert_eq!(ir::NAME, "ir");
    }
}
