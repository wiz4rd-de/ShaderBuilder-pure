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

/// Two CustomSnippet nodes **each declaring a like-named local** (`float t;`)
/// chained through the SSA stream. The generated wrapper functions must keep each
/// body's locals in its own scope so the two `float t;` declarations do not
/// collide — the #43 collision acceptance test. Snippet A reddens via a local
/// `t`; snippet B greens via its own local `t`; both feed an add.
fn fixture_two_snippets_like_named_locals() -> Fixture {
    let body_a = "float t = 0.5;\nout_a = vec4(in_a.r + t, in_a.g, in_a.b, in_a.a);";
    let body_b = "float t = 0.25;\nout_b = vec4(in_b.r, in_b.g + t, in_b.b, in_b.a);";
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
                "snipA",
                NodeOp::CustomSnippet {
                    body: body_a.to_owned(),
                    inputs: vec![PortDecl::new("in_a", PortType::Vec4)],
                    outputs: vec![PortDecl::new("out_a", PortType::Vec4)],
                },
            ),
            IrNode::new(
                "snipB",
                NodeOp::CustomSnippet {
                    body: body_b.to_owned(),
                    inputs: vec![PortDecl::new("in_b", PortType::Vec4)],
                    outputs: vec![PortDecl::new("out_b", PortType::Vec4)],
                },
            ),
            IrNode::new(
                "add",
                NodeOp::Expr {
                    op: ExprOp::Add,
                    operands: vec!["a".to_owned(), "b".to_owned()],
                },
            ),
            IrNode::new("output", NodeOp::Output),
        ],
        edges: vec![
            IrEdge::new("uv", "out", "src", "coord"),
            IrEdge::new("src", "out", "snipA", "in_a"),
            IrEdge::new("src", "out", "snipB", "in_b"),
            IrEdge::new("snipA", "out_a", "add", "a"),
            IrEdge::new("snipB", "out_b", "add", "b"),
            IrEdge::new("add", "out", "output", "color"),
        ],
    };
    Fixture {
        name: "two_snippets_like_named_locals",
        graph,
        ctx: CheckContext::new(),
        opts: EmitOptions {
            name: Some("two_snippets".to_owned()),
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

/// A scalar `Const(Float)` wired straight into `Output.color`. `Output.color` is a
/// `vec4` sink, so the documented `Float→vecN` broadcast applies: the type checker
/// accepts the edge and the emitter must broadcast the scalar at the `FragColor`
/// write (`FragColor = vec4(<temp>);`), not write `FragColor = <float>;` (illegal
/// GLSL). This fixture proves that broadcast path emits + compiles.
fn fixture_scalar_to_output_broadcast() -> Fixture {
    let graph = IrGraph {
        nodes: vec![
            IrNode::new(
                "gray",
                NodeOp::Const {
                    value: ConstValue::Float { value: 0.5 },
                },
            ),
            IrNode::new("output", NodeOp::Output),
        ],
        edges: vec![IrEdge::new("gray", "out", "output", "color")],
    };
    Fixture {
        name: "scalar_to_output_broadcast",
        graph,
        ctx: CheckContext::new(),
        opts: EmitOptions {
            name: Some("gray".to_owned()),
            format: None,
            parameters: vec![],
        },
    }
}

/// `add(Float, FrameCount[int]) → vec4 sample mul → Output`. Exercises the
/// Int-among-scalars arithmetic the checker now permits (`float + int` is legal
/// GLSL via int→float promotion): the `add` lowers to a Float temp emitted as
/// `(t_f + int(params.FrameCount))`, which must compile. Proves the Int operand
/// decision keeps the clean-checks ⇒ compiles invariant end-to-end.
fn fixture_int_scalar_arithmetic() -> Fixture {
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
                "half",
                NodeOp::Const {
                    value: ConstValue::Float { value: 0.5 },
                },
            ),
            IrNode::new(
                "fc",
                NodeOp::Builtin {
                    semantic: BuiltinSemantic::FrameCount,
                },
            ),
            // float + int -> float (scalar promotion).
            IrNode::new(
                "add",
                NodeOp::Expr {
                    op: ExprOp::Add,
                    operands: vec!["a".to_owned(), "b".to_owned()],
                },
            ),
            // vec4 * float (the promoted scalar broadcasts into the vector).
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
            IrEdge::new("half", "out", "add", "a"),
            IrEdge::new("fc", "out", "add", "b"),
            IrEdge::new("src", "out", "mul", "a"),
            IrEdge::new("add", "out", "mul", "b"),
            IrEdge::new("mul", "out", "output", "color"),
        ],
    };
    Fixture {
        name: "int_scalar_arithmetic",
        graph,
        ctx: CheckContext::new(),
        opts: EmitOptions::default(),
    }
}

/// Samples BOTH `Original` and `OriginalHistory{index:0}`. These reference the
/// SAME RetroArch texture (`OriginalHistory0` ≡ `Original`, §5) and emit the same
/// slang identifier `Original`. Lowering must collapse them to ONE manifest
/// sampler so the emitter declares a single `sampler2D Original` — two decls would
/// be a GLSL redefinition that fails to compile. Proves the dedup keeps the
/// clean-checks ⇒ compiles invariant.
fn fixture_original_history_zero_alias() -> Fixture {
    let graph = IrGraph {
        nodes: vec![
            IrNode::new(
                "uv",
                NodeOp::Const {
                    value: ConstValue::Vec2 { value: [0.5, 0.5] },
                },
            ),
            IrNode::new(
                "orig",
                NodeOp::Sample {
                    texture: TextureSource::Original,
                },
            ),
            IrNode::new(
                "hist0",
                NodeOp::Sample {
                    texture: TextureSource::OriginalHistory { index: 0 },
                },
            ),
            IrNode::new(
                "add",
                NodeOp::Expr {
                    op: ExprOp::Add,
                    operands: vec!["a".to_owned(), "b".to_owned()],
                },
            ),
            IrNode::new("output", NodeOp::Output),
        ],
        edges: vec![
            IrEdge::new("uv", "out", "orig", "coord"),
            IrEdge::new("uv", "out", "hist0", "coord"),
            IrEdge::new("orig", "out", "add", "a"),
            IrEdge::new("hist0", "out", "add", "b"),
            IrEdge::new("add", "out", "output", "color"),
        ],
    };
    Fixture {
        name: "original_history_zero_alias",
        graph,
        ctx: CheckContext::new(),
        opts: EmitOptions {
            name: Some("orig_hist0".to_owned()),
            format: None,
            parameters: vec![],
        },
    }
}

fn all_fixtures() -> Vec<Fixture> {
    vec![
        fixture_passthrough_brightness(),
        fixture_param_mix_tint(),
        fixture_multi_sampler(),
        fixture_custom_snippet(),
        fixture_two_snippets_like_named_locals(),
        fixture_builtins(),
        fixture_scalar_to_output_broadcast(),
        fixture_int_scalar_arithmetic(),
        fixture_original_history_zero_alias(),
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

/// Phase-5 graph→IR bridge acceptance (#49): the minimal editor graph
/// `Texcoord → Sample(Source) → Output` must type-check, lower, emit, and COMPILE.
///
/// The frontend `graphToIr` bridge lowers a `Texcoord` node to a `CustomSnippet`
/// with **no inputs** and one `vec2 uv` output whose body reads `vTexCoord` (the
/// raw fragment UV — there is no Builtin texcoord semantic and the IR is frozen).
/// This test pins that design: it reconstructs the EXACT `IrGraph` the bridge
/// emits and proves the `vTexCoord`-in-a-snippet assumption holds — the snippet
/// wrapper function is emitted at fragment-stage file scope where `vTexCoord`
/// (declared `layout(location = 0) in vec2 vTexCoord;`) is in scope, so the
/// generated slang compiles through the real Phase-1 path.
#[test]
fn texcoord_snippet_minimal_graph_compiles() {
    // Mirrors web/src/nodes/{descriptors/coordinates.ts texcoord, graphToIr.ts}.
    let graph = IrGraph {
        nodes: vec![
            IrNode::new(
                "texcoord",
                NodeOp::CustomSnippet {
                    body: "uv = vTexCoord;".to_owned(),
                    inputs: vec![],
                    outputs: vec![PortDecl::new("uv", PortType::Vec2)],
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
            IrEdge::new("texcoord", "uv", "src", "coord"),
            IrEdge::new("src", "out", "output", "color"),
        ],
    };
    let ctx = CheckContext::new();

    // Type-checks clean — the coord source feeds the required Sample.coord input.
    let diags = check(&graph, &ctx);
    assert!(
        !diags.has_errors(),
        "minimal texcoord graph did not type-check clean: {diags:?}"
    );

    let lowered = lower(&graph, &ctx).expect("minimal texcoord graph lowers clean");
    let slang = emit_pass(
        &lowered,
        &EmitOptions {
            name: Some("texcoord_minimal".to_owned()),
            format: None,
            parameters: vec![],
        },
    );

    // The snippet body reads the in-scope `vTexCoord` global from its wrapper fn.
    assert!(
        slang.contains("void snippet_texcoord("),
        "texcoord emitted its own wrapper fn:\n{slang}"
    );
    assert!(
        slang.contains("uv = vTexCoord;"),
        "snippet body reads vTexCoord:\n{slang}"
    );

    // Compiles through the real preprocess → glslang → SPIR-V path: a snippet body
    // referencing an out-of-scope `vTexCoord` would fail here.
    slang_compile::compile_slang(&slang, None).unwrap_or_else(|e| {
        panic!("minimal texcoord graph slang did not compile: {e}\n--- emitted ---\n{slang}")
    });
}

/// Dedup acceptance: a graph sampling both `Original` and `OriginalHistory{0}`
/// (the same RetroArch texture) must emit a SINGLE `sampler2D Original` decl — two
/// would be a GLSL redefinition. Proves the lowering canonicalization reaches the
/// emitter (no duplicate-decl compile failure).
#[test]
fn original_history_zero_alias_emits_a_single_sampler_decl() {
    let fx = fixture_original_history_zero_alias();

    assert!(!check(&fx.graph, &fx.ctx).has_errors());
    let lowered = lower(&fx.graph, &fx.ctx).expect("lowers clean");
    let slang = emit_pass(&lowered, &fx.opts);

    // Compiles through the real path: a duplicate `sampler2D Original` would be a
    // redefinition error here.
    slang_compile::compile_slang(&slang, None).unwrap_or_else(|e| {
        panic!("orig/hist0 slang did not compile: {e}\n--- emitted ---\n{slang}")
    });

    // Exactly one `sampler2D Original;` decl is emitted, not two.
    assert_eq!(
        slang.matches("uniform sampler2D Original;").count(),
        1,
        "Original + OriginalHistory0 must collapse to a single sampler decl:\n{slang}"
    );
}

/// The #43 collision acceptance test: two CustomSnippet nodes that each declare a
/// like-named local (`float t;`) must emit + compile with **no identifier
/// collision**. Each snippet is lowered to its own `snippet_<id>` wrapper
/// function, so the two `float t;` declarations live in separate scopes — the
/// generated slang both compiles through `slang-compile` and contains two
/// distinct wrapper functions (not two `float t;` in one `main`).
#[test]
fn two_snippets_with_like_named_locals_have_no_collision() {
    let fx = fixture_two_snippets_like_named_locals();

    // Clean check + lower + emit.
    assert!(!check(&fx.graph, &fx.ctx).has_errors());
    let lowered = lower(&fx.graph, &fx.ctx).expect("lowers clean");
    let slang = emit_pass(&lowered, &fx.opts);

    // It compiles through the real Phase-1 path — the collision-free proof: a
    // duplicate `float t;` in one scope would be a redeclaration error here.
    slang_compile::compile_slang(&slang, None).unwrap_or_else(|e| {
        panic!("two-snippet slang did not compile: {e}\n--- emitted ---\n{slang}")
    });

    // Both snippet bodies were emitted as their own wrapper functions (so the
    // like-named locals are scoped apart), one per node id.
    assert!(
        slang.contains("void snippet_snipA("),
        "snippet A emitted its own wrapper fn:\n{slang}"
    );
    assert!(
        slang.contains("void snippet_snipB("),
        "snippet B emitted its own wrapper fn:\n{slang}"
    );

    // The two `float t = …;` locals live inside the two wrapper functions, not in
    // `main`: `main` itself declares no `float t`. (Each wrapper carries exactly
    // one `float t`.)
    let main_body = slang
        .rsplit_once("void main()")
        .expect("has a fragment main")
        .1;
    assert!(
        !main_body.contains("float t "),
        "no snippet local leaked into `main`:\n{main_body}"
    );
    assert_eq!(
        slang.matches("float t =").count(),
        2,
        "each wrapper keeps its own `float t` local:\n{slang}"
    );
}

/// #51 Color acceptance: the `Linear → sRGB` colour node (a vec3-port CustomSnippet
/// using the EXACT piecewise transfer) compiles end-to-end. Mirrors the IrGraph the
/// TS bridge emits for `Source → swizzle(.rgb) → linearToSrgb → construct(vec4) →
/// Output`. The snippet body is the verbatim string
/// `web/src/nodes/descriptors/color.ts` generates for `linearToSrgb`.
#[test]
fn color_linear_to_srgb_snippet_compiles() {
    let srgb_body = "vec3 lo = color * 12.92;\n\
         vec3 hi = 1.055 * pow(max(color, vec3(0.0)), vec3(1.0 / 2.4)) - 0.055;\n\
         vec3 cutoff = step(vec3(0.0031308), color);\n\
         result = mix(lo, hi, cutoff);";
    let graph = IrGraph {
        nodes: vec![
            IrNode::new(
                "uv",
                NodeOp::CustomSnippet {
                    body: "uv = vTexCoord;".to_owned(),
                    inputs: vec![],
                    outputs: vec![PortDecl::new("uv", PortType::Vec2)],
                },
            ),
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
                    operands: vec!["in".to_owned()],
                },
            ),
            IrNode::new(
                "srgb",
                NodeOp::CustomSnippet {
                    body: srgb_body.to_owned(),
                    inputs: vec![PortDecl::new("color", PortType::Vec3)],
                    outputs: vec![PortDecl::new("result", PortType::Vec3)],
                },
            ),
            // construct a vec4 from the vec3 + a 1.0 alpha so Output.color (vec4) is fed.
            IrNode::new(
                "one",
                NodeOp::Const {
                    value: ConstValue::Float { value: 1.0 },
                },
            ),
            IrNode::new(
                "rgba",
                NodeOp::Expr {
                    op: ExprOp::Construct { ty: PortType::Vec4 },
                    operands: vec!["a".to_owned(), "b".to_owned()],
                },
            ),
            IrNode::new("output", NodeOp::Output),
        ],
        edges: vec![
            IrEdge::new("uv", "uv", "src", "coord"),
            IrEdge::new("src", "out", "rgb", "in"),
            IrEdge::new("rgb", "out", "srgb", "color"),
            IrEdge::new("srgb", "result", "rgba", "a"),
            IrEdge::new("one", "out", "rgba", "b"),
            IrEdge::new("rgba", "out", "output", "color"),
        ],
    };
    let ctx = CheckContext::new();
    let diags = check(&graph, &ctx);
    assert!(
        !diags.has_errors(),
        "linearToSrgb colour graph did not type-check clean: {diags:?}"
    );
    let lowered = lower(&graph, &ctx).expect("linearToSrgb colour graph lowers clean");
    let slang = emit_pass(
        &lowered,
        &EmitOptions {
            name: Some("color_srgb".to_owned()),
            format: None,
            parameters: vec![],
        },
    );
    // The exact piecewise constants reached the emitted source (not a pow(2.2)).
    assert!(
        slang.contains("0.0031308"),
        "exact sRGB break point emitted:\n{slang}"
    );
    assert!(
        slang.contains("1.0 / 2.4"),
        "exact sRGB exponent emitted:\n{slang}"
    );
    slang_compile::compile_slang(&slang, None).unwrap_or_else(|e| {
        panic!("linearToSrgb colour graph slang did not compile: {e}\n--- emitted ---\n{slang}")
    });
}

/// #51 Sampling acceptance: the `CRT Mask` generator (a CustomSnippet reading a
/// `vec2 uv` + a `vec4 OutputSize` wired from a Builtin OutputSize node) compiles
/// end-to-end, and the mask pitch is driven by `uv * outputSize.xy` so it tracks
/// the simulated-viewport scale. Mirrors the IrGraph the TS bridge emits for
/// `Texcoord + Builtin(OutputSize) → crtMask → mul(Source.rgb) → construct(vec4) →
/// Output`. The snippet body is the verbatim string `sampling.ts` generates for the
/// aperture-grille mask at strength 0.5.
#[test]
fn sampling_crt_mask_snippet_compiles_and_tracks_output_size() {
    let mask_body = "vec2 px = uv * outputSize.xy;\n\
         float phase = mod(px.x, 3.0);\n\
         vec3 m = vec3(step(phase, 1.0), step(1.0, phase) * step(phase, 2.0), step(2.0, phase));\n\
         result = mix(vec3(1.0), m * 3.0, 0.5);";
    let graph = IrGraph {
        nodes: vec![
            IrNode::new(
                "uv",
                NodeOp::CustomSnippet {
                    body: "uv = vTexCoord;".to_owned(),
                    inputs: vec![],
                    outputs: vec![PortDecl::new("uv", PortType::Vec2)],
                },
            ),
            IrNode::new(
                "osize",
                NodeOp::Builtin {
                    semantic: BuiltinSemantic::OutputSize,
                },
            ),
            IrNode::new(
                "mask",
                NodeOp::CustomSnippet {
                    body: mask_body.to_owned(),
                    inputs: vec![
                        PortDecl::new("uv", PortType::Vec2),
                        PortDecl::new("outputSize", PortType::Vec4),
                    ],
                    outputs: vec![PortDecl::new("result", PortType::Vec3)],
                },
            ),
            // Sample the image and tint by the mask, then construct a vec4.
            IrNode::new(
                "uv2",
                NodeOp::CustomSnippet {
                    body: "uv = vTexCoord;".to_owned(),
                    inputs: vec![],
                    outputs: vec![PortDecl::new("uv", PortType::Vec2)],
                },
            ),
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
                    operands: vec!["in".to_owned()],
                },
            ),
            IrNode::new(
                "tint",
                NodeOp::Expr {
                    op: ExprOp::Mul,
                    operands: vec!["a".to_owned(), "b".to_owned()],
                },
            ),
            IrNode::new(
                "one",
                NodeOp::Const {
                    value: ConstValue::Float { value: 1.0 },
                },
            ),
            IrNode::new(
                "rgba",
                NodeOp::Expr {
                    op: ExprOp::Construct { ty: PortType::Vec4 },
                    operands: vec!["a".to_owned(), "b".to_owned()],
                },
            ),
            IrNode::new("output", NodeOp::Output),
        ],
        edges: vec![
            IrEdge::new("uv", "uv", "mask", "uv"),
            IrEdge::new("osize", "out", "mask", "outputSize"),
            IrEdge::new("uv2", "uv", "src", "coord"),
            IrEdge::new("src", "out", "rgb", "in"),
            IrEdge::new("rgb", "out", "tint", "a"),
            IrEdge::new("mask", "result", "tint", "b"),
            IrEdge::new("tint", "out", "rgba", "a"),
            IrEdge::new("one", "out", "rgba", "b"),
            IrEdge::new("rgba", "out", "output", "color"),
        ],
    };
    let ctx = CheckContext::new();
    let diags = check(&graph, &ctx);
    assert!(
        !diags.has_errors(),
        "crtMask sampling graph did not type-check clean: {diags:?}"
    );
    let lowered = lower(&graph, &ctx).expect("crtMask sampling graph lowers clean");
    let slang = emit_pass(
        &lowered,
        &EmitOptions {
            name: Some("crt_mask".to_owned()),
            format: None,
            parameters: vec![],
        },
    );
    // The mask pitch is driven by OutputSize (so it tracks the simulated viewport),
    // and the OutputSize builtin reached the params block.
    assert!(
        slang.contains("uv * outputSize.xy"),
        "mask pitch tracks OutputSize:\n{slang}"
    );
    assert!(
        slang.contains("OutputSize"),
        "OutputSize builtin declared in the params block:\n{slang}"
    );
    slang_compile::compile_slang(&slang, None).unwrap_or_else(|e| {
        panic!("crtMask sampling graph slang did not compile: {e}\n--- emitted ---\n{slang}")
    });
}

/// #51 Color acceptance (YIQ): the `RGB → YIQ → RGB` colour nodes — two vec3-port
/// CustomSnippets emitting column-major NTSC matrices — compile end-to-end. Mirrors
/// the IrGraph the TS bridge emits for `Source → swizzle(.rgb) → rgbToYiq → yiqToRgb
/// → construct(vec4) → Output`. The snippet bodies are the VERBATIM strings
/// `web/src/nodes/descriptors/color.ts` generates (the column-major `mat3(...)`
/// literals from `mat3FromRows`), so this pins that the transposed matrix args
/// compile through the real slang path (a malformed `mat3(...)` would fail here).
#[test]
fn color_rgb_yiq_roundtrip_snippet_compiles() {
    // Column-major (mat3 args are COLUMNS): the row matrix is transposed, exactly as
    // mat3FromRows emits. See web/src/nodes/descriptors/color.ts.
    let fwd_body =
        "result = mat3(0.299, 0.595716, 0.211456, 0.587, -0.274453, -0.522591, 0.114, -0.321263, 0.311135) * color;";
    let inv_body =
        "result = mat3(1.0, 1.0, 1.0, 0.956296, -0.272122, -1.106989, 0.621024, -0.647381, 1.704615) * color;";
    let graph = IrGraph {
        nodes: vec![
            IrNode::new(
                "uv",
                NodeOp::CustomSnippet {
                    body: "uv = vTexCoord;".to_owned(),
                    inputs: vec![],
                    outputs: vec![PortDecl::new("uv", PortType::Vec2)],
                },
            ),
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
                    operands: vec!["in".to_owned()],
                },
            ),
            IrNode::new(
                "fwd",
                NodeOp::CustomSnippet {
                    body: fwd_body.to_owned(),
                    inputs: vec![PortDecl::new("color", PortType::Vec3)],
                    outputs: vec![PortDecl::new("result", PortType::Vec3)],
                },
            ),
            IrNode::new(
                "inv",
                NodeOp::CustomSnippet {
                    body: inv_body.to_owned(),
                    inputs: vec![PortDecl::new("color", PortType::Vec3)],
                    outputs: vec![PortDecl::new("result", PortType::Vec3)],
                },
            ),
            IrNode::new(
                "one",
                NodeOp::Const {
                    value: ConstValue::Float { value: 1.0 },
                },
            ),
            IrNode::new(
                "rgba",
                NodeOp::Expr {
                    op: ExprOp::Construct { ty: PortType::Vec4 },
                    operands: vec!["a".to_owned(), "b".to_owned()],
                },
            ),
            IrNode::new("output", NodeOp::Output),
        ],
        edges: vec![
            IrEdge::new("uv", "uv", "src", "coord"),
            IrEdge::new("src", "out", "rgb", "in"),
            IrEdge::new("rgb", "out", "fwd", "color"),
            IrEdge::new("fwd", "result", "inv", "color"),
            IrEdge::new("inv", "result", "rgba", "a"),
            IrEdge::new("one", "out", "rgba", "b"),
            IrEdge::new("rgba", "out", "output", "color"),
        ],
    };
    let ctx = CheckContext::new();
    let diags = check(&graph, &ctx);
    assert!(
        !diags.has_errors(),
        "rgb↔yiq colour graph did not type-check clean: {diags:?}"
    );
    let lowered = lower(&graph, &ctx).expect("rgb↔yiq colour graph lowers clean");
    let slang = emit_pass(
        &lowered,
        &EmitOptions {
            name: Some("color_yiq".to_owned()),
            format: None,
            parameters: vec![],
        },
    );
    // The column-major matrix args reached the emitted source verbatim.
    assert!(
        slang.contains("mat3(0.299, 0.595716, 0.211456,"),
        "forward YIQ matrix emitted column-major:\n{slang}"
    );
    slang_compile::compile_slang(&slang, None).unwrap_or_else(|e| {
        panic!("rgb↔yiq colour graph slang did not compile: {e}\n--- emitted ---\n{slang}")
    });
}

/// The frontend custom-snippet node (#52, web/src/nodes/descriptors/custom.ts)
/// lowers to a `CustomSnippet` whose author-typed body reads its declared input
/// ports and assigns its declared output ports. This pins the node's DEFAULT body
/// (a colour-clamp pass-through over a `vec4 color` input → `vec4 result` output):
/// the exact IrGraph the bridge emits for `Source → CustomSnippet → Output` must
/// compile through the real slang pipeline (the #52 "a snippet block compiles +
/// previews through compile_graph" acceptance bar).
#[test]
fn custom_snippet_node_default_body_compiles() {
    let graph = IrGraph {
        nodes: vec![
            IrNode::new(
                "uv",
                NodeOp::CustomSnippet {
                    body: "uv = vTexCoord;".to_owned(),
                    inputs: vec![],
                    outputs: vec![PortDecl::new("uv", PortType::Vec2)],
                },
            ),
            IrNode::new(
                "src",
                NodeOp::Sample {
                    texture: TextureSource::Source,
                },
            ),
            // The descriptor's default data: one vec4 `color` in, one vec4 `result`
            // out, body = the clamp pass-through (DEFAULT_BODY in custom.ts).
            IrNode::new(
                "snippet",
                NodeOp::CustomSnippet {
                    body: "result = clamp(color, vec4(0.0), vec4(1.0));".to_owned(),
                    inputs: vec![PortDecl::new("color", PortType::Vec4)],
                    outputs: vec![PortDecl::new("result", PortType::Vec4)],
                },
            ),
            IrNode::new("output", NodeOp::Output),
        ],
        edges: vec![
            IrEdge::new("uv", "uv", "src", "coord"),
            IrEdge::new("src", "out", "snippet", "color"),
            IrEdge::new("snippet", "result", "output", "color"),
        ],
    };
    let ctx = CheckContext::new();
    let diags = check(&graph, &ctx);
    assert!(
        !diags.has_errors(),
        "custom-snippet default-body graph did not type-check clean: {diags:?}"
    );
    let lowered = lower(&graph, &ctx).expect("custom-snippet graph lowers clean");
    let slang = emit_pass(
        &lowered,
        &EmitOptions {
            name: Some("custom_snippet_node".to_owned()),
            format: None,
            parameters: vec![],
        },
    );
    assert!(
        slang.contains("result = clamp(color, vec4(0.0), vec4(1.0));"),
        "snippet body inlined verbatim:\n{slang}"
    );
    slang_compile::compile_slang(&slang, None).unwrap_or_else(|e| {
        panic!("custom-snippet node slang did not compile: {e}\n--- emitted ---\n{slang}")
    });
}
