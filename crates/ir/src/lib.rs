//! `ir` — type-checks a per-pass typed node graph ([`core_model::ir::IrGraph`])
//! and lowers it into a linear, SSA-form [`LoweredIr`] + [`PassManifest`]
//! (Architecture §C).
//!
//! Phase 4 pipeline:
//!
//! 1. [`typecheck`] — a **pure** function of the graph plus a [`CheckContext`]
//!    (no GPU/engine/filesystem) producing node-mapped
//!    [`Diagnostic`](core_model::ir::Diagnostic)s (#40). It is the gate before
//!    lowering.
//! 2. [`lower`](crate::lower::lower) — turns a graph that type-checked **clean**
//!    into the linear SSA form the slang emitter (#42) walks: topologically
//!    ordered [`SsaStmt`]s ending in the `Output` write, plus a deterministic
//!    [`PassManifest`] of the params/builtins/samplers/textures the pass needs
//!    (#41). Lowering runs the checker itself and refuses a graph with errors.
//!
//! [`SsaStmt`]: crate::lower::SsaStmt

pub mod lower;
pub mod typecheck;

pub use lower::{
    lower, LowerContext, LowerError, LoweredIr, LoweredOp, ParamRequirement, PassManifest,
    SamplerBinding, SsaStmt, TempId,
};
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
