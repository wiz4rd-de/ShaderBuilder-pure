//! `preset-io` — `.slangp`/`.slang` import (pipeline reconstruction; passes →
//! whole-pass code nodes; params + LUTs) and the export bundle writer
//! (RetroArch-conventional dir, relative paths, param defaults) (Architecture §B).
//!
//! Phase 0: stub. The round-trip import/export lands in Phase 3 (#…).

/// Crate identity marker. See [`core_model::NAME`].
pub const NAME: &str = "preset-io";

#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {
        assert_eq!(super::NAME, "preset-io");
        // The `preset-io` → `core-model` dependency edge is real and exercised.
        assert_eq!(core_model::NAME, "core-model");
    }
}
