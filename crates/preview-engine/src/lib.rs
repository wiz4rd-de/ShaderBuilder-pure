//! `preview-engine` — the core: a faithful re-implementation of RetroArch's
//! slang runtime on wgpu. Owns the device/queue + source pump on a dedicated
//! render thread, builds the per-pass resource graph (scale types, FBO formats,
//! samplers, feedback double-buffers, history ring, LUTs), computes all builtin
//! semantics, and renders into the simulated viewport (Architecture §D).
//!
//! Phase 0: stub. A single-pass vertical slice lands in Phase 1 (#…); full
//! RetroArch parity in Phase 2 (#…).

/// Crate identity marker. See [`core_model::NAME`].
pub const NAME: &str = "preview-engine";

#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {
        assert_eq!(super::NAME, "preview-engine");
        // The `preview-engine` → `slang-compile` + `source` edges are real and exercised.
        assert_eq!(slang_compile::NAME, "slang-compile");
        assert_eq!(source::NAME, "source");
    }
}
