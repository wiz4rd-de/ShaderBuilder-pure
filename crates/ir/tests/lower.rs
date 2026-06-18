//! Acceptance suite for graph **lowering** (#41).
//!
//! Asserts the three acceptance criteria:
//! - Lowering only accepts a type-checked graph (a type-invalid graph is
//!   *refused*, not silently lowered), and the emitted SSA order respects data
//!   dependencies.
//! - The [`PassManifest`]'s sampled-texture set **exactly** matches the textures
//!   referenced by `Sample` ops in a fixture graph.
//! - Manifest ordering is **deterministic across repeated runs** (snapshot).
//!
//! Every fixture is a small hand-built [`IrGraph`] (Phase-4 graphs are authored in
//! Rust). Lowering is pure, so these run in the headless suite with no GPU.

use core_model::ir::{
    BuiltinSemantic, ConstValue, ExprOp, IrEdge, IrGraph, IrNode, NodeOp, PortDecl, PortType,
    TextureSource,
};
use ir::lower::{LowerError, LoweredOp};
use ir::{lower, CheckContext};

// ----------------------------------------------------------------------------
// Builders (mirrors the type-checker suite's helpers)
// ----------------------------------------------------------------------------

fn konst(id: &str, value: ConstValue) -> IrNode {
    IrNode::new(id, NodeOp::Const { value })
}

fn const_vec2(id: &str) -> IrNode {
    konst(id, ConstValue::Vec2 { value: [0.5, 0.5] })
}

fn const_float(id: &str, v: f32) -> IrNode {
    konst(id, ConstValue::Float { value: v })
}

fn sample(id: &str, texture: TextureSource) -> IrNode {
    IrNode::new(id, NodeOp::Sample { texture })
}

fn expr(id: &str, op: ExprOp, operands: &[&str]) -> IrNode {
    IrNode::new(
        id,
        NodeOp::Expr {
            op,
            operands: operands.iter().map(|s| (*s).to_owned()).collect(),
        },
    )
}

fn param(id: &str, name: &str) -> IrNode {
    IrNode::new(
        id,
        NodeOp::Param {
            name: name.to_owned(),
        },
    )
}

fn builtin(id: &str, semantic: BuiltinSemantic) -> IrNode {
    IrNode::new(id, NodeOp::Builtin { semantic })
}

fn output(id: &str) -> IrNode {
    IrNode::new(id, NodeOp::Output)
}

/// The acceptance demo: `Sample(Source) → mul(sample, brightness) → Output`,
/// with the sample's `coord` fed by a vec2 const.
fn demo_graph() -> IrGraph {
    IrGraph {
        nodes: vec![
            const_vec2("uv"),
            sample("samp", TextureSource::Source),
            const_float("bright", 1.5),
            expr("mul", ExprOp::Mul, &["a", "b"]),
            output("out"),
        ],
        edges: vec![
            IrEdge::new("uv", "out", "samp", "coord"),
            IrEdge::new("samp", "out", "mul", "a"),
            IrEdge::new("bright", "out", "mul", "b"),
            IrEdge::new("mul", "out", "out", "color"),
        ],
    }
}

// ----------------------------------------------------------------------------
// Precondition: lowering refuses a type-invalid graph
// ----------------------------------------------------------------------------

#[test]
fn lowering_refuses_a_type_invalid_graph() {
    // A dangling required input (Sample.coord unfed) is a blocking type error.
    let broken = IrGraph {
        nodes: vec![sample("samp", TextureSource::Source), output("out")],
        edges: vec![IrEdge::new("samp", "out", "out", "color")],
    };
    let result = lower(&broken, &CheckContext::new());
    match result {
        Err(LowerError::TypeErrors(codes)) => {
            assert!(
                codes.iter().any(|c| c == "danglingInput"),
                "expected the dangling-input error to be reported, got {codes:?}"
            );
        }
        other => panic!("type-invalid graph must be refused, got {other:?}"),
    }
}

#[test]
fn lowering_refuses_a_type_mismatch_without_producing_ir() {
    // vec3 into Output.color (vec4) — a type mismatch the checker catches.
    let broken = IrGraph {
        nodes: vec![
            konst(
                "c",
                ConstValue::Vec3 {
                    value: [1.0, 0.0, 0.0],
                },
            ),
            output("out"),
        ],
        edges: vec![IrEdge::new("c", "out", "out", "color")],
    };
    assert!(
        matches!(
            lower(&broken, &CheckContext::new()),
            Err(LowerError::TypeErrors(_))
        ),
        "a type mismatch must be refused, never lowered to garbage IR"
    );
}

// ----------------------------------------------------------------------------
// SSA order respects data dependencies, with the expected typing
// ----------------------------------------------------------------------------

#[test]
fn ssa_order_respects_data_dependencies_and_types() {
    let lowered = lower(&demo_graph(), &CheckContext::new()).expect("clean graph lowers");

    // Every operand temp must be defined by an EARLIER statement (the defining
    // property of a valid SSA linearization of the DAG).
    let mut defined = std::collections::HashSet::new();
    for stmt in &lowered.stmts {
        for operand in &stmt.operands {
            assert!(
                defined.contains(operand),
                "operand {operand} used in stmt {:?} before it was defined; stmts = {:#?}",
                stmt.result,
                lowered.stmts
            );
        }
        assert!(
            defined.insert(stmt.result),
            "temp {:?} defined twice (SSA violated)",
            stmt.result
        );
    }

    // The output temp must be the result of one of the statements.
    assert!(
        lowered.stmts.iter().any(|s| s.result == lowered.output),
        "output temp {:?} must be produced by a statement",
        lowered.output
    );

    // Expected op kinds: uv (const) and bright (const) precede the sample/mul that
    // consume them; sample precedes mul; mul's result is the output.
    let kinds: Vec<&'static str> = lowered
        .stmts
        .iter()
        .map(|s| match &s.op {
            LoweredOp::Const { .. } => "const",
            LoweredOp::Sample { .. } => "sample",
            LoweredOp::Expr { .. } => "expr",
            LoweredOp::Param { .. } => "param",
            LoweredOp::Builtin { .. } => "builtin",
            LoweredOp::CustomSnippet { .. } => "snippet",
        })
        .collect();
    // There are exactly four producing statements (Output is not a statement).
    assert_eq!(lowered.stmts.len(), 4, "kinds = {kinds:?}");
    // The mul (expr) is last and is the output.
    assert_eq!(kinds.last().copied(), Some("expr"));
    assert_eq!(lowered.stmts.last().unwrap().result, lowered.output);

    // Typing: the sample is vec4, the mul (vec4 * float) is vec4.
    let sample_stmt = lowered
        .stmts
        .iter()
        .find(|s| matches!(s.op, LoweredOp::Sample { .. }))
        .unwrap();
    assert_eq!(sample_stmt.ty, PortType::Vec4);
    let mul_stmt = lowered
        .stmts
        .iter()
        .find(|s| matches!(s.op, LoweredOp::Expr { op: ExprOp::Mul }))
        .unwrap();
    assert_eq!(
        mul_stmt.ty,
        PortType::Vec4,
        "vec4 * float broadcasts to vec4"
    );
    // The mul's first operand is the sample (vec4), second the brightness (float).
    assert_eq!(mul_stmt.operands.len(), 2);
    assert_eq!(mul_stmt.operands[0], sample_stmt.result);
}

// ----------------------------------------------------------------------------
// Dead nodes are dropped
// ----------------------------------------------------------------------------

#[test]
fn unreachable_nodes_are_dropped() {
    // `stray` is a const not wired toward Output; it must not be lowered.
    let graph = IrGraph {
        nodes: vec![
            const_vec2("uv"),
            sample("samp", TextureSource::Source),
            const_float("stray", 9.0), // dead
            output("out"),
        ],
        edges: vec![
            IrEdge::new("uv", "out", "samp", "coord"),
            IrEdge::new("samp", "out", "out", "color"),
        ],
    };
    let lowered = lower(&graph, &CheckContext::new()).expect("clean graph lowers");
    // Only uv + samp are emitted (stray dropped); Output is not a statement.
    assert_eq!(lowered.stmts.len(), 2, "stmts = {:#?}", lowered.stmts);
    assert!(
        !lowered
            .stmts
            .iter()
            .any(|s| matches!(&s.op, LoweredOp::Const { value: ConstValue::Float { value } } if *value == 9.0)),
        "the dead `stray` const must be dropped"
    );
}

// ----------------------------------------------------------------------------
// Manifest: sampled-texture set EXACTLY matches the Sample ops
// ----------------------------------------------------------------------------

#[test]
fn manifest_sampled_texture_set_matches_sample_ops_exactly() {
    // Sample Source, Original, PassOutput0, a LUT, and Source again (dup) — the
    // manifest set must be the four DISTINCT textures, deduplicated.
    let graph = IrGraph {
        nodes: vec![
            const_vec2("uv"),
            sample("s0", TextureSource::Source),
            sample("s1", TextureSource::Original),
            sample("s2", TextureSource::PassOutput { index: 0 }),
            sample(
                "s3",
                TextureSource::Lut {
                    name: "BORDER".to_owned(),
                },
            ),
            sample("s4", TextureSource::Source), // duplicate of s0
            // Combine the four distinct samples + the dup into the output via adds.
            expr("a0", ExprOp::Add, &["x", "y"]),
            expr("a1", ExprOp::Add, &["x", "y"]),
            expr("a2", ExprOp::Add, &["x", "y"]),
            expr("a3", ExprOp::Add, &["x", "y"]),
            output("out"),
        ],
        edges: vec![
            IrEdge::new("uv", "out", "s0", "coord"),
            IrEdge::new("uv", "out", "s1", "coord"),
            IrEdge::new("uv", "out", "s2", "coord"),
            IrEdge::new("uv", "out", "s3", "coord"),
            IrEdge::new("uv", "out", "s4", "coord"),
            IrEdge::new("s0", "out", "a0", "x"),
            IrEdge::new("s1", "out", "a0", "y"),
            IrEdge::new("a0", "out", "a1", "x"),
            IrEdge::new("s2", "out", "a1", "y"),
            IrEdge::new("a1", "out", "a2", "x"),
            IrEdge::new("s3", "out", "a2", "y"),
            IrEdge::new("a2", "out", "a3", "x"),
            IrEdge::new("s4", "out", "a3", "y"),
            IrEdge::new("a3", "out", "out", "color"),
        ],
    };
    let lowered = lower(&graph, &CheckContext::new().with_lut("BORDER")).expect("lowers clean");

    // The deduplicated set, in canonical order: Source(0), Original(1),
    // PassOutput0(3), Lut(5).
    let expected = vec![
        TextureSource::Source,
        TextureSource::Original,
        TextureSource::PassOutput { index: 0 },
        TextureSource::Lut {
            name: "BORDER".to_owned(),
        },
    ];
    assert_eq!(
        lowered.manifest.textures, expected,
        "sampled-texture set must exactly match the distinct Sample ops, in canonical order"
    );

    // Samplers mirror the texture set with sequential bindings 0..N.
    assert_eq!(lowered.manifest.samplers.len(), 4);
    for (i, binding) in lowered.manifest.samplers.iter().enumerate() {
        assert_eq!(binding.binding, i as u32);
        assert_eq!(binding.texture, expected[i]);
    }
}

// ----------------------------------------------------------------------------
// Manifest: params + builtins collected and ordered
// ----------------------------------------------------------------------------

#[test]
fn manifest_collects_params_and_builtins() {
    // Two params (GAMMA, BLEND) + two builtins (FrameCount, SourceSize), wired so
    // both reach Output. Ordering must be name/slang-name sorted, not visit order.
    let graph = IrGraph {
        nodes: vec![
            const_vec2("uv"),
            sample("samp", TextureSource::Source),
            param("g", "GAMMA"),
            param("b", "BLEND"),
            builtin("fc", BuiltinSemantic::FrameCount),
            builtin("ss", BuiltinSemantic::SourceSize),
            // pow(sample, gamma) — vec4 ^ float
            expr("pw", ExprOp::Pow, &["x", "y"]),
            // mul by blend (float) and by frame-derived scalar to keep them live.
            expr("m1", ExprOp::Mul, &["x", "y"]),
            // ss.x to get a float from SourceSize; fc widens via add.
            expr(
                "ssx",
                ExprOp::Swizzle {
                    mask: "x".to_owned(),
                },
                &["v"],
            ),
            expr("addf", ExprOp::Add, &["x", "y"]),
            expr("m2", ExprOp::Mul, &["x", "y"]),
            output("out"),
        ],
        edges: vec![
            IrEdge::new("uv", "out", "samp", "coord"),
            IrEdge::new("samp", "out", "pw", "x"),
            IrEdge::new("g", "out", "pw", "y"),
            IrEdge::new("pw", "out", "m1", "x"),
            IrEdge::new("b", "out", "m1", "y"),
            IrEdge::new("ss", "out", "ssx", "v"),
            IrEdge::new("ssx", "out", "addf", "x"),
            IrEdge::new("fc", "out", "addf", "y"),
            IrEdge::new("m1", "out", "m2", "x"),
            IrEdge::new("addf", "out", "m2", "y"),
            IrEdge::new("m2", "out", "out", "color"),
        ],
    };
    let ctx = CheckContext::new()
        .with_parameter("GAMMA")
        .with_parameter("BLEND");
    let lowered = lower(&graph, &ctx).expect("lowers clean");

    // Params sorted by name: BLEND, GAMMA.
    let param_names: Vec<&str> = lowered
        .manifest
        .parameters
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(param_names, vec!["BLEND", "GAMMA"]);

    // Builtins sorted by slang name: FrameCount, SourceSize.
    assert_eq!(
        lowered.manifest.builtins,
        vec![BuiltinSemantic::FrameCount, BuiltinSemantic::SourceSize]
    );
}

// ----------------------------------------------------------------------------
// Snapshot: manifest ordering is deterministic across repeated runs and across
// graphs that differ only in node/edge listing order
// ----------------------------------------------------------------------------

/// A graph sampling several textures, declared in a "messy" node order, used to
/// prove the manifest ordering does not depend on listing order.
fn mixed_textures_graph(reversed: bool) -> IrGraph {
    let mut samples = vec![
        sample(
            "sLut",
            TextureSource::Lut {
                name: "B".to_owned(),
            },
        ),
        sample("sFeed", TextureSource::PassFeedback { index: 2 }),
        sample("sHist", TextureSource::OriginalHistory { index: 1 }),
        sample("sOrig", TextureSource::Original),
        sample("sSrc", TextureSource::Source),
        sample("sOut", TextureSource::PassOutput { index: 0 }),
    ];
    if reversed {
        samples.reverse();
    }
    // Chain them through adds into Output.
    let sample_ids: Vec<String> = samples.iter().map(|n| n.id.clone()).collect();
    let mut nodes = vec![const_vec2("uv")];
    nodes.extend(samples);

    let mut edges: Vec<IrEdge> = sample_ids
        .iter()
        .map(|id| IrEdge::new("uv", "out", id.clone(), "coord"))
        .collect();

    // Fold the samples: acc = ((((s0+s1)+s2)+s3)+s4)+s5
    let mut acc = sample_ids[0].clone();
    for (i, id) in sample_ids.iter().enumerate().skip(1) {
        let add_id = format!("add{i}");
        nodes.push(expr(&add_id, ExprOp::Add, &["x", "y"]));
        edges.push(IrEdge::new(acc.clone(), "out", add_id.clone(), "x"));
        edges.push(IrEdge::new(id.clone(), "out", add_id.clone(), "y"));
        acc = add_id;
    }
    nodes.push(output("out"));
    edges.push(IrEdge::new(acc, "out", "out", "color"));

    IrGraph { nodes, edges }
}

#[test]
fn manifest_ordering_is_deterministic_and_listing_order_independent() {
    let ctx = CheckContext::new().with_lut("B");

    // The canonical expected order (kind, then index/name): Source, Original,
    // OriginalHistory1, PassOutput0, PassFeedback2, Lut(B).
    let expected_textures = vec![
        TextureSource::Source,
        TextureSource::Original,
        TextureSource::OriginalHistory { index: 1 },
        TextureSource::PassOutput { index: 0 },
        TextureSource::PassFeedback { index: 2 },
        TextureSource::Lut {
            name: "B".to_owned(),
        },
    ];

    // 1. Repeated runs over the SAME graph produce byte-identical manifests.
    let a = lower(&mixed_textures_graph(false), &ctx).expect("lowers");
    let b = lower(&mixed_textures_graph(false), &ctx).expect("lowers");
    assert_eq!(a.manifest, b.manifest, "repeated runs must be identical");

    // 2. A graph that differs only in node/edge LISTING order produces the same
    //    manifest (the snapshot the ordering rule guarantees).
    let reversed = lower(&mixed_textures_graph(true), &ctx).expect("lowers");
    assert_eq!(
        a.manifest, reversed.manifest,
        "manifest must not depend on how the graph lists its nodes"
    );

    // 3. The exact snapshot of the texture order + sampler bindings.
    assert_eq!(a.manifest.textures, expected_textures);
    let binding_pairs: Vec<(u32, &TextureSource)> = a
        .manifest
        .samplers
        .iter()
        .map(|s| (s.binding, &s.texture))
        .collect();
    let expected_pairs: Vec<(u32, &TextureSource)> = expected_textures
        .iter()
        .enumerate()
        .map(|(i, t)| (i as u32, t))
        .collect();
    assert_eq!(binding_pairs, expected_pairs);
}

// ----------------------------------------------------------------------------
// CustomSnippet lowering: one result temp per output port, operands in order
// ----------------------------------------------------------------------------

#[test]
fn custom_snippet_lowers_with_operand_and_result_temps() {
    let graph = IrGraph {
        nodes: vec![
            const_vec2("uv"),
            sample("samp", TextureSource::Source),
            IrNode::new(
                "snip",
                NodeOp::CustomSnippet {
                    body: "out_c = in_c.bgra;".to_owned(),
                    inputs: vec![PortDecl::new("in_c", PortType::Vec4)],
                    outputs: vec![PortDecl::new("out_c", PortType::Vec4)],
                },
            ),
            output("out"),
        ],
        edges: vec![
            IrEdge::new("uv", "out", "samp", "coord"),
            IrEdge::new("samp", "out", "snip", "in_c"),
            IrEdge::new("snip", "out_c", "out", "color"),
        ],
    };
    let lowered = lower(&graph, &CheckContext::new()).expect("lowers clean");

    let snip = lowered
        .stmts
        .iter()
        .find(|s| matches!(s.op, LoweredOp::CustomSnippet { .. }))
        .expect("snippet statement present");
    assert_eq!(snip.ty, PortType::Vec4);
    // Its single operand is the sample's vec4.
    assert_eq!(snip.operands.len(), 1);
    let sample_stmt = lowered
        .stmts
        .iter()
        .find(|s| matches!(s.op, LoweredOp::Sample { .. }))
        .unwrap();
    assert_eq!(snip.operands[0], sample_stmt.result);
    // The snippet's result_port output is what feeds FragColor.
    assert_eq!(snip.result, lowered.output);
    if let LoweredOp::CustomSnippet {
        node_id,
        result_port,
        inputs,
        ..
    } = &snip.op
    {
        assert_eq!(node_id, "snip");
        assert_eq!(result_port, "out_c");
        assert_eq!(
            inputs,
            &vec![PortDecl::new("in_c", PortType::Vec4)],
            "lowered snippet carries its typed input ports (for the wrapper signature)"
        );
    } else {
        unreachable!();
    }
}
