//! `slang-compile` — RetroArch-faithful slang preprocessing
//! (`#pragma stage/name/format/parameter`, `#include`, alias), VS/FS split, then
//! glslang → SPIR-V, behind a content-hash shader cache (Architecture §D).
//!
//! Phase 0: stub. The compile pipeline is the make-or-break work in Phase 1 (#…).

/// Crate identity marker. See [`core_model::NAME`].
pub const NAME: &str = "slang-compile";

#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {
        assert_eq!(super::NAME, "slang-compile");
        // The `slang-compile` → `core-model` dependency edge is real and exercised.
        assert_eq!(core_model::NAME, "core-model");
    }
}
