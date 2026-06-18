//! `ir` — type-checks a per-pass typed node graph ([`core_model::ir::IrGraph`])
//! and (in #41) lowers it into a typed dataflow DAG (SSA-style), producing
//! node-mapped [`Diagnostic`](core_model::ir::Diagnostic)s (Architecture §C).
//!
//! Phase 4: the type checker ([`typecheck`]) is the first piece. It is a **pure**
//! function of the graph plus a [`CheckContext`] (no GPU/engine/filesystem), so
//! it runs in the headless test suite; lowering (#41) only lowers a graph that
//! type-checked clean. See the [`typecheck`] module docs for the validation set.

pub mod typecheck;

pub use typecheck::{check, codes, CheckContext};

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
