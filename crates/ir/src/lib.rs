//! `ir` — lowers a per-pass node graph into a typed dataflow DAG (SSA-style),
//! type-checks it, and produces diagnostics (Architecture §C).
//!
//! Phase 0: stub. Lowering + type checking land in Phase 4 (#…).

/// Crate identity marker. See [`core_model::NAME`].
pub const NAME: &str = "ir";

#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {
        assert_eq!(super::NAME, "ir");
        // The `ir` → `core-model` dependency edge is real and exercised.
        assert_eq!(core_model::NAME, "core-model");
    }
}
