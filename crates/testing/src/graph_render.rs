//! Graph → slang → preview parity harness (#44; the Phase-4 exit gate).
//!
//! [`render_graph_to_image`] is the heart of the parity gate: it takes a
//! **hand-built** typed node graph ([`core_model::ir::IrGraph`]), runs it through
//! the full Phase-4 headless codegen pipeline —
//!
//! 1. **type-check** ([`ir::check`], #40) — must be clean,
//! 2. **lower** to SSA + a [`PassManifest`](ir::PassManifest) ([`ir::lower`], #41),
//! 3. **emit** RetroArch Vulkan-GLSL slang ([`codegen_slang::emit_pass`], #42),
//!
//! — and then renders the generated slang through the **already-proven Phase-1/2
//! preview engine**, reusing the exact [`render_preset_to_image`] path the golden
//! suite uses. The simplest proven route (and the one the brief prescribes) is to
//! write the generated slang to a temp dir as a single-pass `.slangp` + `.slang`
//! bundle, then call [`render_preset_to_image`] on it. That reuses the whole
//! compile → chain → render → read-back machinery — no new rendering code.
//!
//! The Phase-4 acceptance criterion (#44) is **parity**: a graph rendered this way
//! must match a *hand-written* equivalent `.slang` rendered through the same path,
//! within the golden suite's diff tolerance. If they match, the IR + emitter are
//! validated end-to-end against a known-good renderer. The parity tests live in
//! `tests/graph_parity.rs`; this module is the reusable machinery they call.
//!
//! ## Why a CustomSnippet `vTexCoord` reader feeds the sampler coord
//!
//! The emitter's [`NodeOp::Sample`](core_model::ir::NodeOp::Sample) takes its
//! coordinate from operand 0 (a `vec2` temp). To make the generated shader sample
//! the *real per-fragment screen UV* — so the render varies across the viewport
//! and the diff against a hand-written `texture(Source, vTexCoord)` is meaningful —
//! a graph feeds the coord from a [`CustomSnippet`](core_model::ir::NodeOp::CustomSnippet)
//! whose one-line body is `out_uv = vTexCoord;`. The snippet's wrapper function is
//! emitted at fragment-stage file scope, where the `layout(location = 0) in vec2
//! vTexCoord` is visible, so the snippet reads the genuine interpolated UV. This is
//! the canonical "feed real screen UV into a graph" idiom for Phase 4, and it
//! doubles as a CustomSnippet exercise. See [`screen_uv_node`].

use std::path::PathBuf;

use codegen_slang::{emit_pass, EmitOptions};
use core_model::ir::{IrGraph, IrNode, NodeOp, PortDecl, PortType};
use image::RgbaImage;
use ir::{check, lower, CheckContext};
use source::Frame;

use crate::{render_preset_to_image, HarnessError};

/// A parameter override `(name, value)` to apply to the rendered preset, the same
/// shape [`render_preset_to_image`] honors via the preset's
/// `parameter_overrides`. Used by the param-driven parity fixtures to push a live
/// value through the params UBO and assert the output responds.
pub type ParamOverride = (String, f32);

/// Errors from [`render_graph_to_image`]: either the graph failed the codegen
/// pipeline (type-check / lower) or the generated-slang render failed.
#[derive(Debug)]
pub enum GraphRenderError {
    /// The graph did not type-check clean — it has blocking [`Diagnostic`]
    /// errors. Carries the offending diagnostic codes (checker order) so a failing
    /// fixture names exactly which rule it broke.
    ///
    /// [`Diagnostic`]: core_model::ir::Diagnostic
    TypeCheck(Vec<String>),
    /// Lowering the (clean) graph failed. Carries the [`ir::LowerError`] display.
    Lower(String),
    /// Rendering the generated `.slangp` through the engine failed (compile,
    /// LUT, or GPU/read-back), wrapping the underlying [`HarnessError`].
    Render(HarnessError),
    /// Writing the temp `.slangp` / `.slang` bundle failed.
    Io(String),
}

impl std::fmt::Display for GraphRenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GraphRenderError::TypeCheck(codes) => {
                write!(f, "graph did not type-check clean: [{}]", codes.join(", "))
            }
            GraphRenderError::Lower(e) => write!(f, "graph lowering failed: {e}"),
            GraphRenderError::Render(e) => write!(f, "generated-slang render failed: {e}"),
            GraphRenderError::Io(e) => write!(f, "temp bundle write failed: {e}"),
        }
    }
}

impl std::error::Error for GraphRenderError {}

/// The single-pass `.slangp` body the harness writes for a generated (or
/// hand-written) `.slang`. Both the generated and the hand-written sides of a
/// parity test render through *this same* preset shape, so the only difference
/// under test is the `.slang` source — never the chain configuration.
///
/// `scale_type0 = viewport` / `scale0 = 1.0` renders the single pass at the full
/// viewport (so a 32×32 viewport yields a 32×32 render target, exactly like the
/// golden multipass fixture's final pass), and `filter_linear0 = true` matches
/// the standard bilinear sampling a hand-written shader expects. These keys are
/// fixed and identical on both sides so they cancel in the diff.
fn single_pass_slangp(slang_filename: &str) -> String {
    format!(
        "shaders = 1\n\
         shader0 = {slang_filename}\n\
         scale_type0 = viewport\n\
         scale0 = 1.0\n\
         filter_linear0 = true\n",
    )
}

/// Write a single-pass `.slangp` + `.slang` bundle into `dir` and render it over
/// `source` at `viewport` / `frame_index`, applying `overrides` as preset
/// `parameter_overrides`. The bundle's `.slangp` is the fixed
/// [`single_pass_slangp`] shape so only the `.slang` source varies between the
/// generated and hand-written sides of a parity comparison.
fn render_slang_bundle(
    dir: &std::path::Path,
    slang_source: &str,
    overrides: &[ParamOverride],
    source: &Frame,
    viewport: (u32, u32),
    frame_index: u64,
) -> Result<RgbaImage, GraphRenderError> {
    let slang_name = "pass.slang";
    let slang_path = dir.join(slang_name);
    let slangp_path = dir.join("preset.slangp");
    std::fs::write(&slang_path, slang_source).map_err(|e| GraphRenderError::Io(e.to_string()))?;

    let mut slangp = single_pass_slangp(slang_name);
    // Apply parameter overrides through the preset's `parameters = "A;B"` +
    // `A = value` form — the same path `render_preset_to_image` parses into the
    // engine's `ApplyParameterOverrides`, so a live parameter value flows through
    // the params UBO exactly as it would for any preset.
    if !overrides.is_empty() {
        let names = overrides
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>()
            .join(";");
        slangp.push_str(&format!("parameters = \"{names}\"\n"));
        for (name, value) in overrides {
            slangp.push_str(&format!("{name} = {value}\n"));
        }
    }
    std::fs::write(&slangp_path, &slangp).map_err(|e| GraphRenderError::Io(e.to_string()))?;

    render_preset_to_image(&slangp_path, source, viewport, frame_index)
        .map_err(GraphRenderError::Render)
}

/// Type-check, lower, and emit slang for `graph`, then render the generated slang
/// through the proven preview engine as a single-pass preset.
///
/// This is the generated-side render of a #44 parity test. The pipeline is the
/// real Phase-4 headless codegen path (`ir::check` → `ir::lower` →
/// `codegen_slang::emit_pass`); the render reuses [`render_preset_to_image`] over
/// a temp `.slangp`/`.slang` bundle (no bespoke rendering).
///
/// * `graph` — the hand-built typed graph (exactly one reachable `Output`).
/// * `ctx` — the declared parameters / LUTs the graph's `Param`/`Sample(Lut)`
///   nodes resolve against (shared by the checker and lowering).
/// * `opts` — the emit options (pass alias / FBO format / the full `#pragma
///   parameter` declarations).
/// * `overrides` — preset parameter overrides applied at render time (the
///   param-driven fixtures push a live value here).
/// * `source` / `viewport` / `frame_index` — the fixed render inputs, matching the
///   golden suite's determinism convention.
///
/// Errors short-circuit: a graph that fails type-check returns
/// [`GraphRenderError::TypeCheck`] (with the offending codes) rather than emitting
/// garbage slang.
pub fn render_graph_to_image(
    graph: &IrGraph,
    ctx: &CheckContext,
    opts: &EmitOptions,
    overrides: &[ParamOverride],
    source: &Frame,
    viewport: (u32, u32),
    frame_index: u64,
) -> Result<RgbaImage, GraphRenderError> {
    // 1. Type-check — refuse a graph with blocking errors (the lowering would too,
    //    but reporting the codes here gives a failing fixture an actionable name).
    let diags = check(graph, ctx);
    if diags.has_errors() {
        let codes = diags
            .iter()
            .filter(|d| d.severity == core_model::ir::DiagnosticSeverity::Error)
            .map(|d| d.code.clone())
            .collect();
        return Err(GraphRenderError::TypeCheck(codes));
    }

    // 2. Lower to SSA + manifest.
    let lowered = lower(graph, ctx).map_err(|e| GraphRenderError::Lower(e.to_string()))?;

    // 3. Emit RetroArch Vulkan-GLSL slang.
    let slang = emit_pass(&lowered, opts);

    // 4. Render the generated slang through the proven engine.
    let dir = tempfile::tempdir().map_err(|e| GraphRenderError::Io(e.to_string()))?;
    render_slang_bundle(dir.path(), &slang, overrides, source, viewport, frame_index)
}

/// Render a **hand-written** `.slang` source through the same single-pass path as
/// [`render_graph_to_image`], so a parity test diffs like against like (only the
/// `.slang` source differs; the `.slangp` chain config and render inputs are
/// identical). This is the reference side of a #44 parity comparison.
pub fn render_handwritten_slang(
    slang_source: &str,
    overrides: &[ParamOverride],
    source: &Frame,
    viewport: (u32, u32),
    frame_index: u64,
) -> Result<RgbaImage, GraphRenderError> {
    let dir = tempfile::tempdir().map_err(|e| GraphRenderError::Io(e.to_string()))?;
    render_slang_bundle(
        dir.path(),
        slang_source,
        overrides,
        source,
        viewport,
        frame_index,
    )
}

/// The canonical "real per-fragment screen UV" source node: a
/// [`CustomSnippet`](NodeOp::CustomSnippet) whose body reads the fragment stage's
/// interpolated `vTexCoord` into a `vec2` output port. Wire its `out_port` to a
/// `Sample` node's `coord` input to sample at the genuine screen UV (so the
/// generated render varies across the viewport, matching a hand-written
/// `texture(Source, vTexCoord)`).
///
/// `id` is the node id (must be unique in the graph); the produced output port is
/// named `out_port`. Returns the node and its output port name for convenient
/// edge wiring.
pub fn screen_uv_node(id: &str, out_port: &str) -> IrNode {
    IrNode::new(
        id,
        NodeOp::CustomSnippet {
            body: format!("{out_port} = vTexCoord;"),
            inputs: Vec::new(),
            outputs: vec![PortDecl::new(out_port, PortType::Vec2)],
        },
    )
}

/// A convenience used by the parity fixtures: the on-disk directory holding the
/// committed hand-written `.slang` equivalents (`fixtures/graph_parity/`). Kept
/// here so both the harness and the test file agree on the location.
pub fn parity_fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("graph_parity")
}
