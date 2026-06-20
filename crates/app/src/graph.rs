//! The `compile_graph` Tauri command (#42): the headless typed-graph → slang
//! compile pipeline, exposed over IPC.
//!
//! It runs the Phase-4 pipeline end to end on one typed [`IrGraph`]:
//! type-check (#40) → lower (#41) → emit slang (#42), and returns a
//! [`CompileGraphResult`] — the emitted `source` (only when the graph is clean
//! and emits successfully) plus the type-checker `diagnostics` (always). When the
//! graph has blocking errors it returns the diagnostics with `source: None` —
//! never an `Err`, so the webview always gets a structured payload to render.
//!
//! ## Phase-4 vs Phase-5 scope
//!
//! In Phase 4 the graph is **hand-built** (in tests) and passed in directly as a
//! typed [`IrGraph`]; the editor-canvas [`core_model::Graph`] → IR bridge and the
//! debounced edit-loop IPC wiring to the React frontend are Phase 5. This command
//! is the stable seam that wiring will call: it already does the full
//! check/lower/emit, so Phase 5 only adds the editor-graph lowering in front of it
//! and the debounce behind it.

use core_model::ir::{CompileGraphResult, IrGraph};
use core_model::Parameter;

use codegen_slang::{emit_pass, EmitOptions};
use ir::{check, lower, CheckContext};

/// Type-check, lower, and emit slang for a single typed [`IrGraph`] (#42).
///
/// `parameters` are the pass's declared `#pragma parameter`s and `luts` the
/// declared LUT names the graph's `Param`/`Sample`-of-LUT references resolve
/// against (Spec §4/§7) — together they build the [`CheckContext`] the checker
/// gates on and supply the `#pragma parameter` lines the emitter writes. `name`
/// is the optional pass alias (`#pragma name`) and `format` the optional FBO
/// format (`#pragma format`).
///
/// Returns a [`CompileGraphResult`]: on a clean graph, the emitted slang in
/// `source` plus the (possibly warning-only) diagnostics; on a graph with
/// blocking errors, `source: None` and the diagnostics. Never an `Err` — the
/// webview always receives a structured payload.
#[tauri::command]
pub fn compile_graph(
    graph: IrGraph,
    parameters: Vec<Parameter>,
    luts: Vec<String>,
    name: Option<String>,
    format: Option<String>,
) -> CompileGraphResult {
    // Build the check/lower context from the declared parameter and LUT names.
    let mut ctx = CheckContext::new();
    for p in &parameters {
        ctx = ctx.with_parameter(&p.name);
    }
    for lut in &luts {
        ctx = ctx.with_lut(lut);
    }

    let diagnostics = check(&graph, &ctx);
    if diagnostics.has_errors() {
        // Blocking errors: surface the diagnostics, no slang.
        return CompileGraphResult {
            source: None,
            diagnostics,
        };
    }

    // Clean enough to lower + emit. `lower` re-runs the checker internally and is
    // infallible on a clean graph (the only non-`TypeErrors` failure is a missing
    // Output, which the checker already reported as an error above). If it does
    // refuse, fall back to no source while keeping the diagnostics.
    let source = match lower(&graph, &ctx) {
        Ok(lowered) => {
            let opts = EmitOptions {
                name,
                format,
                parameters,
            };
            Some(emit_pass(&lowered, &opts))
        }
        Err(_) => None,
    };

    CompileGraphResult {
        source,
        diagnostics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::ir::{ConstValue, ExprOp, IrEdge, IrNode, NodeOp, TextureSource};

    fn param(name: &str) -> Parameter {
        Parameter {
            name: name.to_owned(),
            label: name.to_owned(),
            default: 0.5,
            min: 0.0,
            max: 1.0,
            step: 0.01,
        }
    }

    /// `Sample(Source) → mul by a Param → Output` — a clean graph that emits.
    fn clean_graph() -> IrGraph {
        IrGraph {
            nodes: vec![
                IrNode::new(
                    "uv",
                    NodeOp::Const {
                        value: ConstValue::Vec2 { value: [0.5, 0.5] },
                    },
                ),
                IrNode::new(
                    "src",
                    NodeOp::Sample {
                        texture: TextureSource::Source,
                    },
                ),
                IrNode::new(
                    "amount",
                    NodeOp::Param {
                        name: "GAIN".to_owned(),
                    },
                ),
                IrNode::new(
                    "mul",
                    NodeOp::Expr {
                        op: ExprOp::Mul,
                        operands: vec!["a".to_owned(), "b".to_owned()],
                    },
                ),
                IrNode::new("output", NodeOp::Output),
            ],
            edges: vec![
                IrEdge::new("uv", "out", "src", "coord"),
                IrEdge::new("src", "out", "mul", "a"),
                IrEdge::new("amount", "out", "mul", "b"),
                IrEdge::new("mul", "out", "output", "color"),
            ],
        }
    }

    #[test]
    fn clean_graph_emits_slang_with_no_errors() {
        let result = compile_graph(
            clean_graph(),
            vec![param("GAIN")],
            vec![],
            Some("gain".to_owned()),
            None,
        );
        assert!(!result.diagnostics.has_errors());
        let slang = result.source.expect("clean graph emits slang");
        // The emitted slang carries the pass alias, the parameter, and the
        // Source sampler — and, as the #42 acceptance bar, it compiles.
        assert!(slang.contains("#pragma name gain"));
        assert!(slang.contains("#pragma parameter GAIN"));
        assert!(slang.contains("sampler2D Source"));
        slang_compile::compile_slang(&slang, None)
            .expect("compile_graph output compiles through slang-compile");
    }

    #[test]
    fn unknown_parameter_reports_diagnostics_and_no_slang() {
        // Same graph, but the `GAIN` parameter is not declared in the context.
        let result = compile_graph(clean_graph(), vec![], vec![], None, None);
        assert!(result.diagnostics.has_errors(), "unknown param is an error");
        assert!(
            result.source.is_none(),
            "no slang emitted for an errored graph"
        );
        assert!(
            result.diagnostics.iter().any(|d| d.code == "unknownParam"),
            "carries the unknownParam diagnostic"
        );
    }
}
