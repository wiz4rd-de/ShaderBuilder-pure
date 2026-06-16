//! `core-model` — the single shared serialization contract.
//!
//! Defines the `Project / Pass / Graph / Node / Parameter` model once as Rust
//! `serde` types and exports matching TypeScript so IPC, the native project
//! file, and import/export never drift (Architecture §A).
//!
//! Phase 0: this is a stub. The real schema + TS-type generation land in #12.

/// Crate identity marker. Lets the workspace prove every module is wired in and
/// links before any real implementation exists; replaced by the real public API
/// in later phases.
pub const NAME: &str = "core-model";

#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {
        assert_eq!(super::NAME, "core-model");
    }
}
