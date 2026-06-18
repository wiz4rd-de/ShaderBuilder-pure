//! Graph → slang → preview **parity** suite — the Phase-4 EXIT GATE (#44).
//!
//! For each hand-built fixture graph this:
//!   1. runs the real Phase-4 headless codegen pipeline (type-check #40 → lower
//!      #41 → emit slang #42) and renders the **generated** slang through the
//!      proven preview engine ([`testing::render_graph_to_image`]), and
//!   2. renders a committed **hand-written-equivalent** `.slang`
//!      ([`testing::render_handwritten_slang`]) through the *same* single-pass
//!      path, then
//!   3. golden-image-**diffs** the two renders ([`testing::diff_images`]) with the
//!      golden suite's tolerance convention, writing an amplified diff artifact on
//!      failure.
//!
//! If the generated render matches the hand-written render within tolerance, the
//! IR + emitter are validated end-to-end against a known-good renderer — the phase
//! exit criterion.
//!
//! ## Why the renders are (near-)pixel-identical, not merely "close"
//!
//! Each hand-written `.slang` is authored to compute the **same GLSL expression**
//! the emitter generates from its graph (same operand order, same constructors,
//! same constants), so the two shaders are arithmetically identical and produce
//! byte-identical output on a given adapter (`max_abs == 0`). We still diff with
//! the golden tolerance (TOLERANCE / MAX_FRACTION) rather than asserting exact
//! equality, because the same engine on a *different* GPU adapter perturbs many
//! pixels by a few units (bilinear filtering, software-vs-hardware rounding) — the
//! exact reason `tests/golden.rs` uses a tolerance. The tolerance absorbs adapter
//! noise; a real codegen bug moves a large fraction of pixels far past it.
//!
//! ## Determinism + GPU gating
//!
//! Mirrors `tests/golden.rs` exactly: a FIXED source [`Frame`], a FIXED viewport
//! (32×32), a FIXED frame index, the golden TOLERANCE (12) and MAX_FRACTION
//! (~6%). The tests run unconditionally in the same CI path as the golden suite
//! (`cargo test -p testing`); like the goldens they render on whatever adapter is
//! present (lavapipe in CI, real hardware locally). Both sides render on the SAME
//! adapter, so adapter noise cancels in the diff.

use std::path::{Path, PathBuf};

use codegen_slang::EmitOptions;
use core_model::ir::{
    ConstValue, ExprOp, IrEdge, IrGraph, IrNode, NodeOp, PortDecl, PortType, TextureSource,
};
use core_model::Parameter;
use image::RgbaImage;
use ir::CheckContext;
use source::Frame;
use testing::{
    diff_image, diff_images, render_graph_to_image, render_handwritten_slang, screen_uv_node,
    ParamOverride,
};

// ---- Determinism + tolerance conventions, copied from tests/golden.rs so the
// parity suite behaves identically to the golden suite on every adapter. ----

/// Fixed viewport every parity fixture renders at (matches `tests/golden.rs`).
const VIEWPORT: (u32, u32) = (32, 32);
/// Per-channel tolerance (matches `tests/golden.rs`): absorbs adapter noise.
const TOLERANCE: u8 = 12;
/// Max fraction of pixels allowed over `TOLERANCE` (matches `tests/golden.rs`).
const MAX_FRACTION: f64 = 0.06;
/// Amplification for the visual diff artifact written on failure.
const DIFF_AMPLIFY: u16 = 8;
/// Fixed frame index. Frame 0 is enough — none of the parity fixtures depend on
/// feedback/history accumulation.
const FRAME_INDEX: u64 = 0;

/// The committed hand-written `.slang` equivalents.
fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("graph_parity")
}

/// Failure diffs land under `target/` (gitignored) at a deterministic path so CI
/// can upload them, exactly like the golden suite.
fn artifacts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("graph-parity-artifacts")
}

/// The FIXED source frame, byte-for-byte identical to `tests/golden.rs`'s
/// `fixed_source`: an 8×8 RGBA8 left-red / right-green split with a vertical blue
/// ramp. Non-trivial yet tiny and reproducible.
fn fixed_source() -> Frame {
    let (w, h) = (8u32, 8u32);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let r = if x < w / 2 { 200 } else { 20 };
            let g = if x < w / 2 { 20 } else { 200 };
            let b = (y * 255 / (h - 1)) as u8;
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
    }
    Frame::new(w, h, rgba)
}

fn param(name: &str, label: &str, default: f32, min: f32, max: f32, step: f32) -> Parameter {
    Parameter {
        name: name.to_owned(),
        label: label.to_owned(),
        default,
        min,
        max,
        step,
    }
}

/// A parity fixture: the hand-built graph, its check/lower context, emit options,
/// any parameter overrides applied at render time, and the committed hand-written
/// `.slang` equivalent (by file name under `fixtures/graph_parity/`).
struct Fixture {
    /// Names the diff artifact + identifies the fixture in failure messages.
    name: &'static str,
    graph: IrGraph,
    ctx: CheckContext,
    opts: EmitOptions,
    overrides: Vec<ParamOverride>,
    /// The committed hand-written-equivalent `.slang` file name.
    handwritten: &'static str,
    /// The frame index to render at. Defaults to [`FRAME_INDEX`] (0); a builtin
    /// fixture reading `FrameCount` renders at a `> 0` index so the live counter
    /// value (and its `int(...)` cast) actually perturbs the image.
    frame_index: u64,
}

/// Render the generated graph and the hand-written equivalent through the same
/// path, then diff. Panics with an artifact path on divergence.
fn assert_parity(fx: &Fixture) {
    let src = fixed_source();

    let generated = render_graph_to_image(
        &fx.graph,
        &fx.ctx,
        &fx.opts,
        &fx.overrides,
        &src,
        VIEWPORT,
        fx.frame_index,
    )
    .unwrap_or_else(|e| panic!("fixture `{}` generated-slang render: {e}", fx.name));

    let hand_src =
        std::fs::read_to_string(fixtures_dir().join(fx.handwritten)).unwrap_or_else(|e| {
            panic!(
                "fixture `{}` missing hand-written equivalent {}: {e}",
                fx.name, fx.handwritten
            )
        });
    let handwritten =
        render_handwritten_slang(&hand_src, &fx.overrides, &src, VIEWPORT, fx.frame_index)
            .unwrap_or_else(|e| panic!("fixture `{}` hand-written render: {e}", fx.name));

    let report = diff_images(&generated, &handwritten, TOLERANCE, MAX_FRACTION);
    if !report.passed {
        let dir = artifacts_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = generated.save(dir.join(format!("{}.generated.png", fx.name)));
        let _ = handwritten.save(dir.join(format!("{}.handwritten.png", fx.name)));
        let _ = diff_image(&generated, &handwritten, DIFF_AMPLIFY)
            .save(dir.join(format!("{}.diff.png", fx.name)));
        panic!(
            "parity `{}` diverged: generated vs hand-written max_abs={} mean_abs={:.3} \
             pct_over={:.4} (tol {TOLERANCE}, max_fraction {MAX_FRACTION}). Artifacts in {}.",
            fx.name,
            report.max_abs,
            report.mean_abs,
            report.pct_pixels_over_threshold,
            dir.display()
        );
    }
}

// ----------------------------------------------------------------------------
// Fixtures — each a (Rust-built graph) + (committed hand-written `.slang`).
// ----------------------------------------------------------------------------

/// COLOR TRANSFORM (invert): `vec4(1.0) - texture(Source, vTexCoord)`. Exercises
/// `Sample`, a `Const` vec4, and an `Expr::Sub`, plus the `screen_uv` CustomSnippet
/// feeding the real per-fragment UV.
fn fixture_color_invert() -> Fixture {
    let graph = IrGraph {
        nodes: vec![
            screen_uv_node("uv", "out_uv"),
            IrNode::new(
                "src",
                NodeOp::Sample {
                    texture: TextureSource::Source,
                },
            ),
            IrNode::new(
                "white",
                NodeOp::Const {
                    value: ConstValue::Vec4 {
                        value: [1.0, 1.0, 1.0, 1.0],
                    },
                },
            ),
            IrNode::new(
                "invert",
                NodeOp::Expr {
                    op: ExprOp::Sub,
                    operands: vec!["a".to_owned(), "b".to_owned()],
                },
            ),
            IrNode::new("output", NodeOp::Output),
        ],
        edges: vec![
            IrEdge::new("uv", "out_uv", "src", "coord"),
            IrEdge::new("white", "out", "invert", "a"),
            IrEdge::new("src", "out", "invert", "b"),
            IrEdge::new("invert", "out", "output", "color"),
        ],
    };
    Fixture {
        name: "color_invert",
        graph,
        ctx: CheckContext::new(),
        opts: EmitOptions {
            name: Some("invert".to_owned()),
            format: None,
            parameters: vec![],
        },
        overrides: vec![],
        handwritten: "color_invert.slang",
        frame_index: FRAME_INDEX,
    }
}

/// UV WARP (zoom-toward-center curvature): sample `Source` at a coord pulled
/// toward the center. The warp is computed by a CustomSnippet so the generated
/// shader samples the real, transformed per-fragment UV — exercising `Sample` at
/// a transformed coordinate. `warped = center + (vTexCoord - center) * 0.8`.
fn fixture_uv_warp() -> Fixture {
    let warp_body = "vec2 c = vec2(0.5, 0.5);\nout_uv = c + (vTexCoord - c) * 0.8;";
    let graph = IrGraph {
        nodes: vec![
            IrNode::new(
                "warp",
                NodeOp::CustomSnippet {
                    body: warp_body.to_owned(),
                    inputs: Vec::new(),
                    outputs: vec![PortDecl::new("out_uv", PortType::Vec2)],
                },
            ),
            IrNode::new(
                "src",
                NodeOp::Sample {
                    texture: TextureSource::Source,
                },
            ),
            IrNode::new("output", NodeOp::Output),
        ],
        edges: vec![
            IrEdge::new("warp", "out_uv", "src", "coord"),
            IrEdge::new("src", "out", "output", "color"),
        ],
    };
    Fixture {
        name: "uv_warp",
        graph,
        ctx: CheckContext::new(),
        opts: EmitOptions {
            name: Some("warp".to_owned()),
            format: None,
            parameters: vec![],
        },
        overrides: vec![],
        handwritten: "uv_warp.slang",
        frame_index: FRAME_INDEX,
    }
}

/// PARAMETERIZED CONTRAST/GAMMA: a `#pragma parameter`-driven contrast applied to
/// the sampled color: `(color - 0.5) * CONTRAST + 0.5`. Exercises `Param` flowing
/// through the params UBO, plus `Sub`/`Mul`/`Add` over a `Const` and the param.
/// The default CONTRAST (1.3) is emitted in the `#pragma parameter` line; the
/// param-OVERRIDE variant below pushes a live value.
fn fixture_param_contrast(name: &'static str, overrides: Vec<ParamOverride>) -> Fixture {
    let graph = IrGraph {
        nodes: vec![
            screen_uv_node("uv", "out_uv"),
            IrNode::new(
                "src",
                NodeOp::Sample {
                    texture: TextureSource::Source,
                },
            ),
            IrNode::new(
                "half",
                NodeOp::Const {
                    value: ConstValue::Float { value: 0.5 },
                },
            ),
            IrNode::new(
                "contrast",
                NodeOp::Param {
                    name: "CONTRAST".to_owned(),
                },
            ),
            // (color - 0.5)
            IrNode::new(
                "centered",
                NodeOp::Expr {
                    op: ExprOp::Sub,
                    operands: vec!["a".to_owned(), "b".to_owned()],
                },
            ),
            // (color - 0.5) * CONTRAST
            IrNode::new(
                "scaled",
                NodeOp::Expr {
                    op: ExprOp::Mul,
                    operands: vec!["a".to_owned(), "b".to_owned()],
                },
            ),
            // ((color - 0.5) * CONTRAST) + 0.5
            IrNode::new(
                "recentered",
                NodeOp::Expr {
                    op: ExprOp::Add,
                    operands: vec!["a".to_owned(), "b".to_owned()],
                },
            ),
            IrNode::new("output", NodeOp::Output),
        ],
        edges: vec![
            IrEdge::new("uv", "out_uv", "src", "coord"),
            IrEdge::new("src", "out", "centered", "a"),
            IrEdge::new("half", "out", "centered", "b"),
            IrEdge::new("centered", "out", "scaled", "a"),
            IrEdge::new("contrast", "out", "scaled", "b"),
            IrEdge::new("scaled", "out", "recentered", "a"),
            IrEdge::new("half", "out", "recentered", "b"),
            IrEdge::new("recentered", "out", "output", "color"),
        ],
    };
    Fixture {
        name,
        graph,
        ctx: CheckContext::new().with_parameter("CONTRAST"),
        opts: EmitOptions {
            name: Some("contrast".to_owned()),
            format: None,
            parameters: vec![param("CONTRAST", "Contrast", 1.3, 0.0, 2.0, 0.01)],
        },
        overrides,
        handwritten: "param_contrast.slang",
        frame_index: FRAME_INDEX,
    }
}

/// N-TAP BLUR: a 3-tap horizontal box blur — sample `Source` at three offset
/// coords and average. Exercises MULTIPLE `Sample` ops at offset coords, `Add`,
/// and a scalar `Mul`. The three offset coords are produced by CustomSnippets so
/// the taps sample real per-fragment UVs. `(s(uv-d) + s(uv) + s(uv+d)) / 3`.
fn fixture_blur() -> Fixture {
    // One texel step at the 8×8 source: 1/8 = 0.125 in UV. A wide, deterministic
    // offset so the three taps land on visibly different source columns.
    let dx = "0.125";
    let left_body = format!("out_uv = vTexCoord - vec2({dx}, 0.0);");
    let center_body = "out_uv = vTexCoord;".to_owned();
    let right_body = format!("out_uv = vTexCoord + vec2({dx}, 0.0);");
    let graph = IrGraph {
        nodes: vec![
            IrNode::new(
                "uvL",
                NodeOp::CustomSnippet {
                    body: left_body,
                    inputs: Vec::new(),
                    outputs: vec![PortDecl::new("out_uv", PortType::Vec2)],
                },
            ),
            IrNode::new(
                "uvC",
                NodeOp::CustomSnippet {
                    body: center_body,
                    inputs: Vec::new(),
                    outputs: vec![PortDecl::new("out_uv", PortType::Vec2)],
                },
            ),
            IrNode::new(
                "uvR",
                NodeOp::CustomSnippet {
                    body: right_body,
                    inputs: Vec::new(),
                    outputs: vec![PortDecl::new("out_uv", PortType::Vec2)],
                },
            ),
            IrNode::new(
                "sL",
                NodeOp::Sample {
                    texture: TextureSource::Source,
                },
            ),
            IrNode::new(
                "sC",
                NodeOp::Sample {
                    texture: TextureSource::Source,
                },
            ),
            IrNode::new(
                "sR",
                NodeOp::Sample {
                    texture: TextureSource::Source,
                },
            ),
            // sL + sC
            IrNode::new(
                "sum1",
                NodeOp::Expr {
                    op: ExprOp::Add,
                    operands: vec!["a".to_owned(), "b".to_owned()],
                },
            ),
            // (sL + sC) + sR
            IrNode::new(
                "sum2",
                NodeOp::Expr {
                    op: ExprOp::Add,
                    operands: vec!["a".to_owned(), "b".to_owned()],
                },
            ),
            IrNode::new(
                "third",
                NodeOp::Const {
                    value: ConstValue::Float { value: 1.0 / 3.0 },
                },
            ),
            // ((sL + sC) + sR) * (1/3)
            IrNode::new(
                "avg",
                NodeOp::Expr {
                    op: ExprOp::Mul,
                    operands: vec!["a".to_owned(), "b".to_owned()],
                },
            ),
            IrNode::new("output", NodeOp::Output),
        ],
        edges: vec![
            IrEdge::new("uvL", "out_uv", "sL", "coord"),
            IrEdge::new("uvC", "out_uv", "sC", "coord"),
            IrEdge::new("uvR", "out_uv", "sR", "coord"),
            IrEdge::new("sL", "out", "sum1", "a"),
            IrEdge::new("sC", "out", "sum1", "b"),
            IrEdge::new("sum1", "out", "sum2", "a"),
            IrEdge::new("sR", "out", "sum2", "b"),
            IrEdge::new("sum2", "out", "avg", "a"),
            IrEdge::new("third", "out", "avg", "b"),
            IrEdge::new("avg", "out", "output", "color"),
        ],
    };
    Fixture {
        name: "blur",
        graph,
        ctx: CheckContext::new(),
        opts: EmitOptions {
            name: Some("blur".to_owned()),
            format: None,
            parameters: vec![],
        },
        overrides: vec![],
        handwritten: "blur.slang",
        frame_index: FRAME_INDEX,
    }
}

/// CUSTOM SNIPPET (channel mix): a verbatim GLSL body that swaps R and B
/// (`vec4(in_color.b, in_color.g, in_color.r, in_color.a)`). Exercises the
/// `CustomSnippet` op end-to-end — the snippet gains the sampled color via an
/// `in` port, assigns its `out` port, and the emitter wraps it in a dedicated
/// function whose locals are scoped apart from `main`.
fn fixture_snippet_channel_swap() -> Fixture {
    let body = "out_color = vec4(in_color.b, in_color.g, in_color.r, in_color.a);";
    let graph = IrGraph {
        nodes: vec![
            screen_uv_node("uv", "out_uv"),
            IrNode::new(
                "src",
                NodeOp::Sample {
                    texture: TextureSource::Source,
                },
            ),
            IrNode::new(
                "swap",
                NodeOp::CustomSnippet {
                    body: body.to_owned(),
                    inputs: vec![PortDecl::new("in_color", PortType::Vec4)],
                    outputs: vec![PortDecl::new("out_color", PortType::Vec4)],
                },
            ),
            IrNode::new("output", NodeOp::Output),
        ],
        edges: vec![
            IrEdge::new("uv", "out_uv", "src", "coord"),
            IrEdge::new("src", "out", "swap", "in_color"),
            IrEdge::new("swap", "out_color", "output", "color"),
        ],
    };
    Fixture {
        name: "snippet_channel_swap",
        graph,
        ctx: CheckContext::new(),
        opts: EmitOptions {
            name: Some("channel_swap".to_owned()),
            format: None,
            parameters: vec![],
        },
        overrides: vec![],
        handwritten: "snippet_channel_swap.slang",
        frame_index: FRAME_INDEX,
    }
}

/// SWIZZLE (`.bgra`): sample `Source` then swizzle `.bgra` into the output —
/// reverses R<->B, keeps G/A. The swizzle mask is operand-order-sensitive, so a
/// wrong mask moves pixels (the red/green source split flips channels). Exercises
/// `ExprOp::Swizzle` end-to-end. `texture(Source, vTexCoord).bgra`.
fn fixture_swizzle_bgra() -> Fixture {
    let graph = IrGraph {
        nodes: vec![
            screen_uv_node("uv", "out_uv"),
            IrNode::new(
                "src",
                NodeOp::Sample {
                    texture: TextureSource::Source,
                },
            ),
            IrNode::new(
                "sw",
                NodeOp::Expr {
                    op: ExprOp::Swizzle {
                        mask: "bgra".to_owned(),
                    },
                    operands: vec!["v".to_owned()],
                },
            ),
            IrNode::new("output", NodeOp::Output),
        ],
        edges: vec![
            IrEdge::new("uv", "out_uv", "src", "coord"),
            IrEdge::new("src", "out", "sw", "v"),
            IrEdge::new("sw", "out", "output", "color"),
        ],
    };
    Fixture {
        name: "swizzle_bgra",
        graph,
        ctx: CheckContext::new(),
        opts: EmitOptions {
            name: Some("swizzle_bgra".to_owned()),
            format: None,
            parameters: vec![],
        },
        overrides: vec![],
        handwritten: "swizzle_bgra.slang",
        frame_index: FRAME_INDEX,
    }
}

/// CONSTRUCT (vec4 from vec3 + scalar): sample `Source`, take `.rgb` (a vec3),
/// then `Construct(vec4)` from (that vec3, a const float alpha 1.0). The
/// constructor's operand ORDER and grouping matter, so a wrong order moves
/// pixels. Exercises `ExprOp::Construct` (the explicit-widening path) plus a
/// `Swizzle`. `vec4(texture(Source, vTexCoord).rgb, 1.0)`.
fn fixture_construct_vec4() -> Fixture {
    let graph = IrGraph {
        nodes: vec![
            screen_uv_node("uv", "out_uv"),
            IrNode::new(
                "src",
                NodeOp::Sample {
                    texture: TextureSource::Source,
                },
            ),
            IrNode::new(
                "rgb",
                NodeOp::Expr {
                    op: ExprOp::Swizzle {
                        mask: "rgb".to_owned(),
                    },
                    operands: vec!["v".to_owned()],
                },
            ),
            IrNode::new(
                "alpha",
                NodeOp::Const {
                    value: ConstValue::Float { value: 1.0 },
                },
            ),
            IrNode::new(
                "ctor",
                NodeOp::Expr {
                    op: ExprOp::Construct { ty: PortType::Vec4 },
                    operands: vec!["xyz".to_owned(), "w".to_owned()],
                },
            ),
            IrNode::new("output", NodeOp::Output),
        ],
        edges: vec![
            IrEdge::new("uv", "out_uv", "src", "coord"),
            IrEdge::new("src", "out", "rgb", "v"),
            IrEdge::new("rgb", "out", "ctor", "xyz"),
            IrEdge::new("alpha", "out", "ctor", "w"),
            IrEdge::new("ctor", "out", "output", "color"),
        ],
    };
    Fixture {
        name: "construct_vec4",
        graph,
        ctx: CheckContext::new(),
        opts: EmitOptions {
            name: Some("construct_vec4".to_owned()),
            format: None,
            parameters: vec![],
        },
        overrides: vec![],
        handwritten: "construct_vec4.slang",
        frame_index: FRAME_INDEX,
    }
}

/// MIX (operand order): `mix(color, blue, 0.25)` — 75% source + 25% blend.
/// `mix(a, b, t)` is order-sensitive (it is `a*(1-t)+b*t`), so swapping a/b or a
/// wrong `t` moves pixels. Exercises `ExprOp::Mix` with vector operands and a
/// scalar `t`. `mix(texture(Source, vTexCoord), vec4(0,0,1,1), 0.25)`.
fn fixture_mix_blend() -> Fixture {
    let graph = IrGraph {
        nodes: vec![
            screen_uv_node("uv", "out_uv"),
            IrNode::new(
                "src",
                NodeOp::Sample {
                    texture: TextureSource::Source,
                },
            ),
            IrNode::new(
                "blend",
                NodeOp::Const {
                    value: ConstValue::Vec4 {
                        value: [0.0, 0.0, 1.0, 1.0],
                    },
                },
            ),
            IrNode::new(
                "t",
                NodeOp::Const {
                    value: ConstValue::Float { value: 0.25 },
                },
            ),
            IrNode::new(
                "mix",
                NodeOp::Expr {
                    op: ExprOp::Mix,
                    operands: vec!["a".to_owned(), "b".to_owned(), "t".to_owned()],
                },
            ),
            IrNode::new("output", NodeOp::Output),
        ],
        edges: vec![
            IrEdge::new("uv", "out_uv", "src", "coord"),
            IrEdge::new("src", "out", "mix", "a"),
            IrEdge::new("blend", "out", "mix", "b"),
            IrEdge::new("t", "out", "mix", "t"),
            IrEdge::new("mix", "out", "output", "color"),
        ],
    };
    Fixture {
        name: "mix_blend",
        graph,
        ctx: CheckContext::new(),
        opts: EmitOptions {
            name: Some("mix_blend".to_owned()),
            format: None,
            parameters: vec![],
        },
        overrides: vec![],
        handwritten: "mix_blend.slang",
        frame_index: FRAME_INDEX,
    }
}

/// CLAMP (operand order): `clamp(color, 0.25, 0.65)` — the band `[lo, hi]` is
/// order-sensitive (lo before hi). With the source channels spanning ~0.08..0.78
/// the band genuinely moves pixels (low channels clamp up, high channels clamp
/// down). Exercises `ExprOp::Clamp` with a vector `x` and scalar bounds.
/// `clamp(texture(Source, vTexCoord), 0.25, 0.65)`.
fn fixture_clamp_band() -> Fixture {
    let graph = IrGraph {
        nodes: vec![
            screen_uv_node("uv", "out_uv"),
            IrNode::new(
                "src",
                NodeOp::Sample {
                    texture: TextureSource::Source,
                },
            ),
            IrNode::new(
                "lo",
                NodeOp::Const {
                    value: ConstValue::Float { value: 0.25 },
                },
            ),
            IrNode::new(
                "hi",
                NodeOp::Const {
                    value: ConstValue::Float { value: 0.65 },
                },
            ),
            IrNode::new(
                "clamp",
                NodeOp::Expr {
                    op: ExprOp::Clamp,
                    operands: vec!["x".to_owned(), "lo".to_owned(), "hi".to_owned()],
                },
            ),
            IrNode::new("output", NodeOp::Output),
        ],
        edges: vec![
            IrEdge::new("uv", "out_uv", "src", "coord"),
            IrEdge::new("src", "out", "clamp", "x"),
            IrEdge::new("lo", "out", "clamp", "lo"),
            IrEdge::new("hi", "out", "clamp", "hi"),
            IrEdge::new("clamp", "out", "output", "color"),
        ],
    };
    Fixture {
        name: "clamp_band",
        graph,
        ctx: CheckContext::new(),
        opts: EmitOptions {
            name: Some("clamp_band".to_owned()),
            format: None,
            parameters: vec![],
        },
        overrides: vec![],
        handwritten: "clamp_band.slang",
        frame_index: FRAME_INDEX,
    }
}

/// DIV (operand order): `color / 2.0` — division is order-sensitive
/// (`a / b` != `b / a`), so a swapped numerator/denominator would compute
/// `2.0 / color` and move pixels far. Exercises `ExprOp::Div` with a vector
/// numerator and scalar denominator. `texture(Source, vTexCoord) / 2.0`.
fn fixture_div_scale() -> Fixture {
    let graph = IrGraph {
        nodes: vec![
            screen_uv_node("uv", "out_uv"),
            IrNode::new(
                "src",
                NodeOp::Sample {
                    texture: TextureSource::Source,
                },
            ),
            IrNode::new(
                "two",
                NodeOp::Const {
                    value: ConstValue::Float { value: 2.0 },
                },
            ),
            IrNode::new(
                "div",
                NodeOp::Expr {
                    op: ExprOp::Div,
                    operands: vec!["a".to_owned(), "b".to_owned()],
                },
            ),
            IrNode::new("output", NodeOp::Output),
        ],
        edges: vec![
            IrEdge::new("uv", "out_uv", "src", "coord"),
            IrEdge::new("src", "out", "div", "a"),
            IrEdge::new("two", "out", "div", "b"),
            IrEdge::new("div", "out", "output", "color"),
        ],
    };
    Fixture {
        name: "div_scale",
        graph,
        ctx: CheckContext::new(),
        opts: EmitOptions {
            name: Some("div_scale".to_owned()),
            format: None,
            parameters: vec![],
        },
        overrides: vec![],
        handwritten: "div_scale.slang",
        frame_index: FRAME_INDEX,
    }
}

/// BUILTIN read (`FrameCount`, at frame_index > 0): a `Builtin(FrameCount)` (a
/// `uint` in the params block, typed `int` in the IR — read through the emitter's
/// `int(...)` cast) drives a scalar `phase = fract(FrameCount * 0.25)`, then the
/// sampled color is scaled by it. Rendered at [`FRAME_COUNT_INDEX`] (= 2, so
/// `FrameCount == 2` ⇒ `phase = fract(0.5) = 0.5`), the live counter VALUE and
/// its cast both perturb the image — a wrong cast or a baked/zero FrameCount
/// would change the result. `texture(...) * fract(int(params.FrameCount) * 0.25)`.
fn fixture_builtin_framecount() -> Fixture {
    let graph = IrGraph {
        nodes: vec![
            screen_uv_node("uv", "out_uv"),
            IrNode::new(
                "src",
                NodeOp::Sample {
                    texture: TextureSource::Source,
                },
            ),
            IrNode::new(
                "fc",
                NodeOp::Builtin {
                    semantic: core_model::ir::BuiltinSemantic::FrameCount,
                },
            ),
            IrNode::new(
                "quarter",
                NodeOp::Const {
                    value: ConstValue::Float { value: 0.25 },
                },
            ),
            // FrameCount (int, widened to float) * 0.25
            IrNode::new(
                "phaseRaw",
                NodeOp::Expr {
                    op: ExprOp::Mul,
                    operands: vec!["a".to_owned(), "b".to_owned()],
                },
            ),
            // fract(FrameCount * 0.25)
            IrNode::new(
                "phase",
                NodeOp::Expr {
                    op: ExprOp::Fract,
                    operands: vec!["x".to_owned()],
                },
            ),
            // color * phase
            IrNode::new(
                "tint",
                NodeOp::Expr {
                    op: ExprOp::Mul,
                    operands: vec!["a".to_owned(), "b".to_owned()],
                },
            ),
            IrNode::new("output", NodeOp::Output),
        ],
        edges: vec![
            IrEdge::new("uv", "out_uv", "src", "coord"),
            IrEdge::new("fc", "out", "phaseRaw", "a"),
            IrEdge::new("quarter", "out", "phaseRaw", "b"),
            IrEdge::new("phaseRaw", "out", "phase", "x"),
            IrEdge::new("src", "out", "tint", "a"),
            IrEdge::new("phase", "out", "tint", "b"),
            IrEdge::new("tint", "out", "output", "color"),
        ],
    };
    Fixture {
        name: "builtin_framecount",
        graph,
        ctx: CheckContext::new(),
        opts: EmitOptions {
            name: Some("builtin_framecount".to_owned()),
            format: None,
            parameters: vec![],
        },
        overrides: vec![],
        handwritten: "builtin_framecount.slang",
        frame_index: FRAME_COUNT_INDEX,
    }
}

/// The frame index the `FrameCount` builtin fixture renders at: a value `> 0` so
/// the live counter perturbs the image. `2` ⇒ `FrameCount == 2` (the engine's
/// post-2-advance state) ⇒ `phase = fract(2 * 0.25) = fract(0.5) = 0.5`.
const FRAME_COUNT_INDEX: u64 = 2;

// ----------------------------------------------------------------------------
// Tests — one per fixture (granular so a failure names the exact op family).
// ----------------------------------------------------------------------------

#[test]
fn parity_color_invert() {
    assert_parity(&fixture_color_invert());
}

#[test]
fn parity_uv_warp() {
    assert_parity(&fixture_uv_warp());
}

#[test]
fn parity_param_contrast_default() {
    // No override: the generated shader reads the `#pragma parameter` default
    // (CONTRAST 1.3) from the params UBO; the hand-written shader declares the
    // same `#pragma parameter`, so both apply the same contrast.
    assert_parity(&fixture_param_contrast("param_contrast_default", vec![]));
}

#[test]
fn parity_blur() {
    assert_parity(&fixture_blur());
}

#[test]
fn parity_snippet_channel_swap() {
    assert_parity(&fixture_snippet_channel_swap());
}

#[test]
fn parity_swizzle_bgra() {
    assert_parity(&fixture_swizzle_bgra());
}

#[test]
fn parity_construct_vec4() {
    assert_parity(&fixture_construct_vec4());
}

#[test]
fn parity_mix_blend() {
    assert_parity(&fixture_mix_blend());
}

#[test]
fn parity_clamp_band() {
    assert_parity(&fixture_clamp_band());
}

#[test]
fn parity_div_scale() {
    assert_parity(&fixture_div_scale());
}

/// The `FrameCount` builtin fixture is rendered at frame_index > 0, so beyond the
/// generated-vs-hand-written parity check we also confirm the live counter VALUE
/// actually moved the pixels: the frame-2 render (phase = 0.5) must differ from a
/// frame-0 render (phase = fract(0) = 0 ⇒ black), proving the FrameCount read is
/// live (and correctly cast) rather than a baked constant. A codegen bug that
/// dropped the cast would fail to compile; one that read a wrong/zero value would
/// fail this differs-from-frame-0 assertion.
#[test]
fn parity_builtin_framecount_is_live() {
    let fx = fixture_builtin_framecount();
    // 1. Generated == hand-written at frame_index = 2.
    assert_parity(&fx);

    // 2. The render at frame 2 (phase 0.5) differs from frame 0 (phase 0, black),
    //    proving the FrameCount value flows live into the shader.
    let src = fixed_source();
    let frame2 = render_graph_to_image(
        &fx.graph,
        &fx.ctx,
        &fx.opts,
        &fx.overrides,
        &src,
        VIEWPORT,
        FRAME_COUNT_INDEX,
    )
    .expect("frame-2 FrameCount render");
    let frame0 = render_graph_to_image(
        &fx.graph,
        &fx.ctx,
        &fx.opts,
        &fx.overrides,
        &src,
        VIEWPORT,
        0,
    )
    .expect("frame-0 FrameCount render");
    assert_differs(
        &frame2,
        &frame0,
        "FrameCount must change the output between frame 0 and frame 2",
    );
}

/// PARAM DRIVEN: confirm a LIVE parameter value flows through the params UBO. With
/// an override of CONTRAST = 1.8 (vs the 1.3 default), the generated graph and the
/// hand-written equivalent — rendered under the SAME override — still match, AND
/// the output differs from the default-contrast render (so the override demonstrably
/// changed the pixels, proving the value is live, not baked).
#[test]
fn parity_param_contrast_override_is_live() {
    let overridden = fixture_param_contrast(
        "param_contrast_override",
        vec![("CONTRAST".to_owned(), 1.8)],
    );
    // 1. Generated == hand-written under the same live override.
    assert_parity(&overridden);

    // 2. The override actually moved the pixels (vs the default render), proving
    //    the parameter value flows live through the UBO rather than being baked.
    let src = fixed_source();
    let default_fx = fixture_param_contrast("param_contrast_default", vec![]);
    let default_render = render_graph_to_image(
        &default_fx.graph,
        &default_fx.ctx,
        &default_fx.opts,
        &default_fx.overrides,
        &src,
        VIEWPORT,
        FRAME_INDEX,
    )
    .expect("default-contrast render");
    let overridden_render = render_graph_to_image(
        &overridden.graph,
        &overridden.ctx,
        &overridden.opts,
        &overridden.overrides,
        &src,
        VIEWPORT,
        FRAME_INDEX,
    )
    .expect("overridden-contrast render");

    assert_differs(
        &default_render,
        &overridden_render,
        "CONTRAST override must change the output vs the default",
    );
}

/// Assert two renders are NOT within the parity tolerance — i.e. a change really
/// took effect. The inverse of [`assert_parity`]'s verdict.
fn assert_differs(a: &RgbaImage, b: &RgbaImage, why: &str) {
    let report = diff_images(a, b, TOLERANCE, MAX_FRACTION);
    assert!(
        !report.passed,
        "{why}: renders were within tolerance (max_abs={}, pct_over={:.4}) — \
         the parameter did not flow through",
        report.max_abs, report.pct_pixels_over_threshold
    );
}
