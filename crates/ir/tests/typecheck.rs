//! Table-driven acceptance suite for the per-pass graph type checker (#40).
//!
//! Two tables:
//! - [`valid_graphs`] — graphs that must type-check **clean** (zero diagnostics).
//! - [`broken_graphs`] — intentionally-broken graphs, each asserting the **exact**
//!   diagnostic code **and the offending node id** (we assert the id, not just
//!   the message — that node-mapping is the whole point of #40).
//!
//! Every fixture is a small hand-built [`IrGraph`] (Phase-4 graphs are authored
//! in Rust). The checker is pure, so these run in the headless suite with no GPU.

use core_model::ir::{
    ConstValue, Diagnostics, ExprOp, IrEdge, IrGraph, IrNode, NodeOp, PortDecl, PortType,
    TextureSource,
};
use ir::{check, codes, CheckContext};

// ----------------------------------------------------------------------------
// Builders
// ----------------------------------------------------------------------------

/// A `Const` source node of the given typed value.
fn konst(id: &str, value: ConstValue) -> IrNode {
    IrNode::new(id, NodeOp::Const { value })
}

fn const_vec2(id: &str) -> IrNode {
    konst(id, ConstValue::Vec2 { value: [0.5, 0.5] })
}

fn const_vec4(id: &str) -> IrNode {
    konst(
        id,
        ConstValue::Vec4 {
            value: [0.1, 0.2, 0.3, 1.0],
        },
    )
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

fn output(id: &str) -> IrNode {
    IrNode::new(id, NodeOp::Output)
}

/// `Sample(Source)` whose `coord` is fed by a vec2 const, into `Output` — the
/// minimal valid graph. Returns the graph; callers can extend.
fn minimal_valid() -> IrGraph {
    IrGraph {
        nodes: vec![
            const_vec2("uv"),
            sample("samp", TextureSource::Source),
            output("out"),
        ],
        edges: vec![
            IrEdge::new("uv", "out", "samp", "coord"),
            IrEdge::new("samp", "out", "out", "color"),
        ],
    }
}

// ----------------------------------------------------------------------------
// Valid graphs — must produce ZERO diagnostics
// ----------------------------------------------------------------------------

#[test]
fn valid_graphs_produce_zero_diagnostics() {
    struct Case {
        name: &'static str,
        graph: IrGraph,
        ctx: CheckContext,
    }

    let cases = vec![
        Case {
            name: "minimal Sample(Source) -> Output",
            graph: minimal_valid(),
            ctx: CheckContext::new(),
        },
        Case {
            // The acceptance demo: Sample -> Expr(color transform) -> Output, with
            // a Float brightness broadcasting into a vec4 multiply.
            name: "Sample -> mul(vec4, float-broadcast) -> Output",
            graph: IrGraph {
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
            },
            ctx: CheckContext::new(),
        },
        Case {
            // A declared Param feeding a mix between two samples.
            name: "mix(sampleA, sampleB, param) with declared param",
            graph: IrGraph {
                nodes: vec![
                    const_vec2("uv"),
                    sample("a", TextureSource::Source),
                    sample("b", TextureSource::Original),
                    IrNode::new(
                        "p",
                        NodeOp::Param {
                            name: "BLEND".to_owned(),
                        },
                    ),
                    expr("mix", ExprOp::Mix, &["x", "y", "t"]),
                    output("out"),
                ],
                edges: vec![
                    IrEdge::new("uv", "out", "a", "coord"),
                    IrEdge::new("uv", "out", "b", "coord"),
                    IrEdge::new("a", "out", "mix", "x"),
                    IrEdge::new("b", "out", "mix", "y"),
                    IrEdge::new("p", "out", "mix", "t"),
                    IrEdge::new("mix", "out", "out", "color"),
                ],
            },
            ctx: CheckContext::new().with_parameter("BLEND"),
        },
        Case {
            // Swizzle + construct: take rgb of a sample, build vec4 with a 1.0.
            name: "construct(vec4) from swizzle(.rgb) + const float",
            graph: IrGraph {
                nodes: vec![
                    const_vec2("uv"),
                    sample("samp", TextureSource::Source),
                    expr(
                        "rgb",
                        ExprOp::Swizzle {
                            mask: "rgb".to_owned(),
                        },
                        &["v"],
                    ),
                    const_float("alpha", 1.0),
                    expr(
                        "ctor",
                        ExprOp::Construct { ty: PortType::Vec4 },
                        &["xyz", "w"],
                    ),
                    output("out"),
                ],
                edges: vec![
                    IrEdge::new("uv", "out", "samp", "coord"),
                    IrEdge::new("samp", "out", "rgb", "v"),
                    IrEdge::new("rgb", "out", "ctor", "xyz"),
                    IrEdge::new("alpha", "out", "ctor", "w"),
                    IrEdge::new("ctor", "out", "out", "color"),
                ],
            },
            ctx: CheckContext::new(),
        },
        Case {
            // Sample a declared LUT.
            name: "Sample(Lut) with declared LUT name",
            graph: IrGraph {
                nodes: vec![
                    const_vec2("uv"),
                    sample(
                        "lut",
                        TextureSource::Lut {
                            name: "BORDER".to_owned(),
                        },
                    ),
                    output("out"),
                ],
                edges: vec![
                    IrEdge::new("uv", "out", "lut", "coord"),
                    IrEdge::new("lut", "out", "out", "color"),
                ],
            },
            ctx: CheckContext::new().with_lut("BORDER"),
        },
        Case {
            // A CustomSnippet with typed in/out ports, correctly wired.
            name: "CustomSnippet vec4->vec4 wired correctly",
            graph: IrGraph {
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
            },
            ctx: CheckContext::new(),
        },
        Case {
            // Int builtin (FrameCount) -> float param-style math is fine via widen.
            name: "dot(vec3, vec3) -> float used in scalar math",
            graph: IrGraph {
                nodes: vec![
                    konst(
                        "a",
                        ConstValue::Vec3 {
                            value: [1.0, 0.0, 0.0],
                        },
                    ),
                    konst(
                        "b",
                        ConstValue::Vec3 {
                            value: [0.0, 1.0, 0.0],
                        },
                    ),
                    expr("d", ExprOp::Dot, &["x", "y"]),
                    expr("ctor", ExprOp::Construct { ty: PortType::Vec4 }, &["s"]),
                    output("out"),
                ],
                edges: vec![
                    IrEdge::new("a", "out", "d", "x"),
                    IrEdge::new("b", "out", "d", "y"),
                    // dot -> float broadcasts to fill the vec4 construct.
                    IrEdge::new("d", "out", "ctor", "s"),
                    IrEdge::new("ctor", "out", "out", "color"),
                ],
            },
            ctx: CheckContext::new(),
        },
    ];

    for case in cases {
        let diags = check(&case.graph, &case.ctx);
        assert!(
            diags.is_empty(),
            "valid graph `{}` should produce zero diagnostics, got: {:?}",
            case.name,
            diags
        );
        assert!(!diags.has_errors());
    }
}

// ----------------------------------------------------------------------------
// Broken graphs — assert the EXACT diagnostic code + offending node id
// ----------------------------------------------------------------------------

/// Assert that `diags` contains exactly one diagnostic with `code` on node `node`
/// (and, if `port` is `Some`, that exact port). Other diagnostics may co-exist
/// only when `exclusive` is false.
#[track_caller]
fn assert_has(diags: &Diagnostics, code: &str, node: &str, port: Option<&str>) {
    let matches: Vec<_> = diags
        .iter()
        .filter(|d| {
            d.code == code && d.node == node && (port.is_none() || d.port.as_deref() == port)
        })
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "expected exactly one `{code}` diagnostic on node `{node}` (port {port:?}); got diagnostics: {:#?}",
        diags
    );
}

#[test]
fn broken_graphs_emit_expected_node_mapped_diagnostics() {
    // --- type mismatch: vec3 into Output.color (which is vec4) -------------
    {
        let graph = IrGraph {
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
        let diags = check(&graph, &CheckContext::new());
        assert_has(&diags, codes::TYPE_MISMATCH, "out", Some("color"));
    }

    // --- cycle: a -> b -> a (plus a valid-looking Output) ------------------
    {
        let graph = IrGraph {
            nodes: vec![
                expr("a", ExprOp::Abs, &["x"]),
                expr("b", ExprOp::Abs, &["x"]),
                output("out"),
            ],
            edges: vec![
                IrEdge::new("a", "out", "b", "x"),
                IrEdge::new("b", "out", "a", "x"),
                IrEdge::new("b", "out", "out", "color"),
            ],
        };
        let diags = check(&graph, &CheckContext::new());
        // Both a and b are on the cycle; the back edge target (a) is flagged.
        assert!(
            diags.iter().any(|d| d.code == codes::CYCLE),
            "expected a cycle diagnostic, got {diags:#?}"
        );
        let cycle_nodes: Vec<&str> = diags
            .iter()
            .filter(|d| d.code == codes::CYCLE)
            .map(|d| d.node.as_str())
            .collect();
        assert!(
            cycle_nodes.contains(&"a") || cycle_nodes.contains(&"b"),
            "cycle diagnostic must name a node on the cycle, got {cycle_nodes:?}"
        );
    }

    // --- missing Output ----------------------------------------------------
    {
        let graph = IrGraph {
            nodes: vec![const_vec2("uv"), sample("samp", TextureSource::Source)],
            edges: vec![IrEdge::new("uv", "out", "samp", "coord")],
        };
        let diags = check(&graph, &CheckContext::new());
        assert!(
            diags.iter().any(|d| d.code == codes::MISSING_OUTPUT),
            "expected missingOutput, got {diags:#?}"
        );
    }

    // --- multiple Outputs: each surplus Output is flagged by id ------------
    {
        let graph = IrGraph {
            nodes: vec![const_vec4("c"), output("out1"), output("out2")],
            edges: vec![
                IrEdge::new("c", "out", "out1", "color"),
                IrEdge::new("c", "out", "out2", "color"),
            ],
        };
        let diags = check(&graph, &CheckContext::new());
        assert_has(&diags, codes::MULTIPLE_OUTPUTS, "out1", None);
        assert_has(&diags, codes::MULTIPLE_OUTPUTS, "out2", None);
    }

    // --- unknown texture (LUT not declared) --------------------------------
    {
        let graph = IrGraph {
            nodes: vec![
                const_vec2("uv"),
                sample(
                    "lut",
                    TextureSource::Lut {
                        name: "MISSING".to_owned(),
                    },
                ),
                output("out"),
            ],
            edges: vec![
                IrEdge::new("uv", "out", "lut", "coord"),
                IrEdge::new("lut", "out", "out", "color"),
            ],
        };
        let diags = check(&graph, &CheckContext::new());
        assert_has(&diags, codes::UNKNOWN_TEXTURE, "lut", None);
    }

    // --- unknown param -----------------------------------------------------
    {
        let graph = IrGraph {
            nodes: vec![
                IrNode::new(
                    "p",
                    NodeOp::Param {
                        name: "NOPE".to_owned(),
                    },
                ),
                expr("ctor", ExprOp::Construct { ty: PortType::Vec4 }, &["s"]),
                output("out"),
            ],
            edges: vec![
                IrEdge::new("p", "out", "ctor", "s"),
                IrEdge::new("ctor", "out", "out", "color"),
            ],
        };
        // ctx declares a *different* parameter, so NOPE is unresolved.
        let diags = check(&graph, &CheckContext::new().with_parameter("GAMMA"));
        assert_has(&diags, codes::UNKNOWN_PARAM, "p", None);
    }

    // --- wrong arity: mix (ternary) wired with two operands ----------------
    {
        let graph = IrGraph {
            nodes: vec![
                const_vec4("a"),
                const_vec4("b"),
                expr("mix", ExprOp::Mix, &["x", "y"]), // missing `t`
                output("out"),
            ],
            edges: vec![
                IrEdge::new("a", "out", "mix", "x"),
                IrEdge::new("b", "out", "mix", "y"),
                IrEdge::new("mix", "out", "out", "color"),
            ],
        };
        let diags = check(&graph, &CheckContext::new());
        assert_has(&diags, codes::WRONG_ARITY, "mix", None);
    }

    // --- illegal swizzle: .z on a vec2 -------------------------------------
    {
        let graph = IrGraph {
            nodes: vec![
                const_vec2("uv"),
                expr(
                    "sw",
                    ExprOp::Swizzle {
                        mask: "z".to_owned(),
                    },
                    &["v"],
                ),
                expr("ctor", ExprOp::Construct { ty: PortType::Vec4 }, &["s"]),
                output("out"),
            ],
            edges: vec![
                IrEdge::new("uv", "out", "sw", "v"),
                IrEdge::new("sw", "out", "ctor", "s"),
                IrEdge::new("ctor", "out", "out", "color"),
            ],
        };
        let diags = check(&graph, &CheckContext::new());
        assert_has(&diags, codes::ILLEGAL_SWIZZLE, "sw", None);
    }

    // --- dangling required input: Sample.coord unfed -----------------------
    {
        let graph = IrGraph {
            nodes: vec![sample("samp", TextureSource::Source), output("out")],
            edges: vec![IrEdge::new("samp", "out", "out", "color")],
        };
        let diags = check(&graph, &CheckContext::new());
        assert_has(&diags, codes::DANGLING_INPUT, "samp", Some("coord"));
    }

    // --- operand type: add(vec2, vec3) incompatible vector widths ----------
    {
        let graph = IrGraph {
            nodes: vec![
                const_vec2("a"),
                konst(
                    "b",
                    ConstValue::Vec3 {
                        value: [1.0, 2.0, 3.0],
                    },
                ),
                expr("add", ExprOp::Add, &["x", "y"]),
                expr("ctor", ExprOp::Construct { ty: PortType::Vec4 }, &["s"]),
                output("out"),
            ],
            edges: vec![
                IrEdge::new("a", "out", "add", "x"),
                IrEdge::new("b", "out", "add", "y"),
                IrEdge::new("add", "out", "ctor", "s"),
                IrEdge::new("ctor", "out", "out", "color"),
            ],
        };
        let diags = check(&graph, &CheckContext::new());
        assert_has(&diags, codes::OPERAND_TYPE, "add", None);
    }

    // --- unknown port: edge targets a non-existent input port --------------
    {
        let graph = IrGraph {
            nodes: vec![
                const_vec2("uv"),
                sample("samp", TextureSource::Source),
                output("out"),
            ],
            edges: vec![
                IrEdge::new("uv", "out", "samp", "coord"),
                // `samp` has no `nope` input port.
                IrEdge::new("uv", "out", "samp", "nope"),
                IrEdge::new("samp", "out", "out", "color"),
            ],
        };
        let diags = check(&graph, &CheckContext::new());
        assert_has(&diags, codes::UNKNOWN_PORT, "samp", Some("nope"));
    }

    // --- unknown node: edge references a missing node ----------------------
    {
        let graph = IrGraph {
            nodes: vec![const_vec4("c"), output("out")],
            edges: vec![
                IrEdge::new("c", "out", "out", "color"),
                IrEdge::new("ghost", "out", "out", "color"),
            ],
        };
        let diags = check(&graph, &CheckContext::new());
        assert_has(&diags, codes::UNKNOWN_NODE, "ghost", None);
    }
}

// ----------------------------------------------------------------------------
// The clean/has-errors distinction lowering (#41) keys off
// ----------------------------------------------------------------------------

#[test]
fn check_distinguishes_clean_from_errors_for_lowering() {
    // Clean graph: no errors, ready to lower.
    let clean = check(&minimal_valid(), &CheckContext::new());
    assert!(clean.is_empty());
    assert!(!clean.has_errors());

    // Broken graph: has_errors() is the gate lowering checks.
    let broken = IrGraph {
        nodes: vec![sample("samp", TextureSource::Source), output("out")],
        edges: vec![IrEdge::new("samp", "out", "out", "color")],
    };
    let diags = check(&broken, &CheckContext::new());
    assert!(
        diags.has_errors(),
        "a dangling required input is a blocking error"
    );
}
