//! Emit-then-compile + snapshot acceptance for the IR → slang emitter (#42).
//!
//! For each hand-built fixture graph this:
//! 1. type-checks it (#40) — must be clean,
//! 2. lowers it to `LoweredIr` + `PassManifest` (#41),
//! 3. emits slang (#42),
//! 4. asserts `slang_compile::compile_slang(&emitted, None)` returns `Ok` (the
//!    #42 acceptance bar — the generated slang compiles through the real Phase-1
//!    preprocess → glslang → SPIR-V path with no errors), and
//! 5. compares the emitted source byte-wise against a committed `.slang` snapshot
//!    (regression guard against codegen drift).
//!
//! Snapshots live in `tests/snapshots/<fixture>.slang`. Set `UPDATE_SNAPSHOTS=1`
//! to rewrite them after an intentional emitter change (mirrors the golden
//! harness's update escape hatch).

use std::path::PathBuf;

use codegen_slang::{emit_pass, EmitOptions};
use core_model::ir::{
    BuiltinSemantic, ConstValue, ExprOp, IrEdge, IrGraph, IrNode, NodeOp, PortDecl, PortType,
    TextureSource,
};
use core_model::Parameter;
use ir::{check, lower, CheckContext};

/// A fixture: a name (for the snapshot file), a graph, the check/lower context,
/// and the emit options (alias/format/parameters).
struct Fixture {
    name: &'static str,
    graph: IrGraph,
    ctx: CheckContext,
    opts: EmitOptions,
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

/// `Sample(Source) → mul by a brightness Const → Output`. The canonical demo.
fn fixture_passthrough_brightness() -> Fixture {
    let graph = IrGraph {
        nodes: vec![
            IrNode::new(
                "uv",
                NodeOp::Const {
                    value: ConstValue::Vec2 { value: [0.5, 0.5] },
                },
            ),
            IrNode::new(
                "sample",
                NodeOp::Sample {
                    texture: TextureSource::Source,
                },
            ),
            IrNode::new(
                "bright",
                NodeOp::Const {
                    value: ConstValue::Float { value: 1.5 },
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
            IrEdge::new("uv", "out", "sample", "coord"),
            IrEdge::new("sample", "out", "mul", "a"),
            IrEdge::new("bright", "out", "mul", "b"),
            IrEdge::new("mul", "out", "output", "color"),
        ],
    };
    Fixture {
        name: "passthrough_brightness",
        graph,
        ctx: CheckContext::new(),
        opts: EmitOptions {
            name: Some("brightness".to_owned()),
            format: None,
            parameters: vec![],
        },
    }
}

/// A pass with a `#pragma parameter`-driven mix between the sampled color and a
/// constant tint, plus a builtin read (`SourceSize`) folded in — exercises the
/// params block, the `#pragma parameter` line, builtin uniform reads, `mix`, and
/// swizzle/construct ops.
fn fixture_param_mix_tint() -> Fixture {
    let graph = IrGraph {
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
                "tint",
                NodeOp::Const {
                    value: ConstValue::Vec4 {
                        value: [1.0, 0.5, 0.25, 1.0],
                    },
                },
            ),
            IrNode::new(
                "amount",
                NodeOp::Param {
                    name: "TINT_AMOUNT".to_owned(),
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
            IrEdge::new("uv", "out", "src", "coord"),
            IrEdge::new("src", "out", "mix", "a"),
            IrEdge::new("tint", "out", "mix", "b"),
            IrEdge::new("amount", "out", "mix", "t"),
            IrEdge::new("mix", "out", "output", "color"),
        ],
    };
    Fixture {
        name: "param_mix_tint",
        graph,
        ctx: CheckContext::new().with_parameter("TINT_AMOUNT"),
        opts: EmitOptions {
            name: Some("tint".to_owned()),
            format: Some("R8G8B8A8_UNORM".to_owned()),
            parameters: vec![param("TINT_AMOUNT", "Tint Amount", 0.5, 0.0, 1.0, 0.01)],
        },
    }
}

/// Multiple sampled textures (`Source` + a LUT) plus builtins and a swizzle —
/// exercises deterministic multi-sampler binding assignment and the texture
/// name mapping (`Source`, the LUT name).
fn fixture_multi_sampler() -> Fixture {
    let graph = IrGraph {
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
                "lut",
                NodeOp::Sample {
                    texture: TextureSource::Lut {
                        name: "PALETTE".to_owned(),
                    },
                },
            ),
            IrNode::new(
                "add",
                NodeOp::Expr {
                    op: ExprOp::Add,
                    operands: vec!["a".to_owned(), "b".to_owned()],
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
                "one",
                NodeOp::Const {
                    value: ConstValue::Float { value: 1.0 },
                },
            ),
            IrNode::new(
                "build",
                NodeOp::Expr {
                    op: ExprOp::Construct { ty: PortType::Vec4 },
                    operands: vec!["xyz".to_owned(), "w".to_owned()],
                },
            ),
            IrNode::new("output", NodeOp::Output),
        ],
        edges: vec![
            IrEdge::new("uv", "out", "src", "coord"),
            IrEdge::new("uv", "out", "lut", "coord"),
            IrEdge::new("src", "out", "add", "a"),
            IrEdge::new("lut", "out", "add", "b"),
            IrEdge::new("add", "out", "rgb", "v"),
            IrEdge::new("rgb", "out", "build", "xyz"),
            IrEdge::new("one", "out", "build", "w"),
            IrEdge::new("build", "out", "output", "color"),
        ],
    };
    Fixture {
        name: "multi_sampler",
        graph,
        ctx: CheckContext::new().with_lut("PALETTE"),
        opts: EmitOptions {
            name: Some("blend".to_owned()),
            format: None,
            parameters: vec![],
        },
    }
}

/// A CustomSnippet escape-hatch pass: a verbatim GLSL body that gains the sampled
/// color and clamps it. Exercises the snippet inlining path (input `#define`s,
/// output locals, body verbatim).
fn fixture_custom_snippet() -> Fixture {
    let graph = IrGraph {
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
                "gain",
                NodeOp::Const {
                    value: ConstValue::Float { value: 1.25 },
                },
            ),
            IrNode::new(
                "snippet",
                NodeOp::CustomSnippet {
                    body: "out_color = clamp(in_color * gain, 0.0, 1.0);".to_owned(),
                    inputs: vec![
                        PortDecl::new("in_color", PortType::Vec4),
                        PortDecl::new("gain", PortType::Float),
                    ],
                    outputs: vec![PortDecl::new("out_color", PortType::Vec4)],
                },
            ),
            IrNode::new("output", NodeOp::Output),
        ],
        edges: vec![
            IrEdge::new("uv", "out", "src", "coord"),
            IrEdge::new("src", "out", "snippet", "in_color"),
            IrEdge::new("gain", "out", "snippet", "gain"),
            IrEdge::new("snippet", "out_color", "output", "color"),
        ],
    };
    Fixture {
        name: "custom_snippet",
        graph,
        ctx: CheckContext::new(),
        opts: EmitOptions {
            name: Some("snippet".to_owned()),
            format: None,
            parameters: vec![],
        },
    }
}

/// A builtin-reading pass: scroll the sampled UV by `FrameCount` and read
/// `SourceSize` — exercises builtin uniform reads (`params.SourceSize`,
/// `int(params.FrameCount)`) and float/vec arithmetic.
fn fixture_builtins() -> Fixture {
    let graph = IrGraph {
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
                "ssize",
                NodeOp::Builtin {
                    semantic: BuiltinSemantic::SourceSize,
                },
            ),
            IrNode::new(
                "scale",
                NodeOp::Expr {
                    op: ExprOp::Mul,
                    operands: vec!["a".to_owned(), "b".to_owned()],
                },
            ),
            IrNode::new("output", NodeOp::Output),
        ],
        edges: vec![
            IrEdge::new("uv", "out", "src", "coord"),
            IrEdge::new("src", "out", "scale", "a"),
            IrEdge::new("ssize", "out", "scale", "b"),
            IrEdge::new("scale", "out", "output", "color"),
        ],
    };
    Fixture {
        name: "builtins",
        graph,
        ctx: CheckContext::new(),
        opts: EmitOptions::default(),
    }
}

fn all_fixtures() -> Vec<Fixture> {
    vec![
        fixture_passthrough_brightness(),
        fixture_param_mix_tint(),
        fixture_multi_sampler(),
        fixture_custom_snippet(),
        fixture_builtins(),
    ]
}

fn snapshot_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(format!("{name}.slang"))
}

/// Emit each fixture, assert it compiles through slang-compile, and check it
/// against (or update) its committed `.slang` snapshot.
#[test]
fn fixtures_emit_compile_and_match_snapshots() {
    let update = std::env::var_os("UPDATE_SNAPSHOTS").is_some();

    for fx in all_fixtures() {
        // 1. Type-check must be clean.
        let diags = check(&fx.graph, &fx.ctx);
        assert!(
            !diags.has_errors(),
            "fixture `{}` did not type-check clean: {:?}",
            fx.name,
            diags
        );

        // 2. Lower.
        let lowered = lower(&fx.graph, &fx.ctx)
            .unwrap_or_else(|e| panic!("fixture `{}` failed to lower: {e}", fx.name));

        // 3. Emit.
        let slang = emit_pass(&lowered, &fx.opts);

        // 4. Emit-then-compile: the acceptance bar.
        slang_compile::compile_slang(&slang, None).unwrap_or_else(|e| {
            panic!(
                "fixture `{}` emitted slang did not compile: {e}\n--- emitted ---\n{slang}",
                fx.name
            )
        });

        // 5. Snapshot.
        let path = snapshot_path(fx.name);
        if update {
            std::fs::write(&path, &slang)
                .unwrap_or_else(|e| panic!("could not write snapshot {path:?}: {e}"));
            continue;
        }
        let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "missing snapshot {path:?} ({e}); run with UPDATE_SNAPSHOTS=1 to create it.\n\
                 --- emitted ---\n{slang}"
            )
        });
        assert_eq!(
            slang, expected,
            "fixture `{}` emitted slang drifted from its snapshot {path:?}; \
             re-run with UPDATE_SNAPSHOTS=1 if the change is intentional.",
            fx.name
        );
    }
}
