//! `source` — the preview's frame pump: still images, built-in test patterns,
//! and PNG sequences in the core (ffmpeg video stays optional/pluggable to keep
//! the core license-clean). Advancing frames drive `FrameCount` / feedback /
//! history (Architecture §D, Decision Log #8/#13).
//!
//! Phase 0: stub. Still images + test patterns + the PNG-sequence pump land in
//! Phase 2 (#…).

/// Crate identity marker. See [`core_model::NAME`].
pub const NAME: &str = "source";

#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {
        assert_eq!(super::NAME, "source");
        // The `source` → `core-model` dependency edge is real and exercised.
        assert_eq!(core_model::NAME, "core-model");
    }
}
