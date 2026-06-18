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
            // Int among scalars: `add(float, int)` is legal GLSL (int promotes to
            // float for scalar arithmetic) and lowers to a Float result, so it
            // must type-check CLEAN. (Only `vector op int` is rejected.)
            name: "add(float, int) scalar promotion is clean",
            graph: IrGraph {
                nodes: vec![
                    const_float("f", 0.5),
                    konst("i", ConstValue::Int { value: 2 }),
                    expr("add", ExprOp::Add, &["x", "y"]),
                    expr("ctor", ExprOp::Construct { ty: PortType::Vec4 }, &["s"]),
                    output("out"),
                ],
                edges: vec![
                    IrEdge::new("f", "out", "add", "x"),
                    IrEdge::new("i", "out", "add", "y"),
                    IrEdge::new("add", "out", "ctor", "s"),
                    IrEdge::new("ctor", "out", "out", "color"),
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
// Broken graphs — assert the EXACT diagnostic set (code + node id + port)
// ----------------------------------------------------------------------------

/// Assert `diags` is **exactly** the expected set of `(code, node, port)` triples
/// — every diagnostic present matches one expected entry and vice versa, with no
/// spurious extras and no missing entries. Matching is multiset-based so a
/// duplicate expectation requires a duplicate diagnostic. A `port` of `None` in an
/// expectation matches a diagnostic regardless of its port; `Some(_)` requires the
/// exact port. This is the strongest false-green guard: it pins both WHICH
/// diagnostics fire and that NOTHING else does.
#[track_caller]
fn assert_only(diags: &Diagnostics, expected: &[(&str, &str, Option<&str>)]) {
    let actual: Vec<(&str, &str, Option<&str>)> = diags
        .iter()
        .map(|d| (d.code.as_str(), d.node.as_str(), d.port.as_deref()))
        .collect();
    assert_eq!(
        actual.len(),
        expected.len(),
        "expected exactly {} diagnostic(s) {:?}, got {} diagnostic(s): {:#?}",
        expected.len(),
        expected,
        actual.len(),
        diags
    );
    // Multiset match: each expected entry consumes one actual diagnostic.
    let mut remaining: Vec<(&str, &str, Option<&str>)> = actual.clone();
    for exp @ (code, node, port) in expected {
        let pos = remaining
            .iter()
            .position(|(c, n, p)| c == code && n == node && (port.is_none() || p == port));
        match pos {
            Some(i) => {
                remaining.swap_remove(i);
            }
            None => panic!(
                "expected diagnostic {exp:?} not found among {:#?} (already matched the rest)",
                diags
            ),
        }
    }
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
        // The lone error is the vec3->vec4 mismatch on the Output's `color` port;
        // nothing else is wrong with the graph.
        assert_only(&diags, &[(codes::TYPE_MISMATCH, "out", Some("color"))]);
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
        // The DFS visits start nodes in node-list order (a, b, out): from `a` it
        // descends a -> b, then b's back edge to the still-`Visiting` `a` flags `a`
        // as the cycle node. The impl sorts `in_cycle` and emits one diagnostic per
        // node, so the result is deterministic: EXACTLY one `cycle` diagnostic, on
        // node `a`, and the type-driven checks are skipped on a cyclic graph — so
        // there must be NO other diagnostics (no spurious extras).
        assert_only(&diags, &[(codes::CYCLE, "a", None)]);
    }

    // --- missing Output ----------------------------------------------------
    {
        let graph = IrGraph {
            nodes: vec![const_vec2("uv"), sample("samp", TextureSource::Source)],
            edges: vec![IrEdge::new("uv", "out", "samp", "coord")],
        };
        let diags = check(&graph, &CheckContext::new());
        // Exactly ONE missingOutput, attributed to the empty node id by design
        // (there is no offending node — it is a graph-level error), and nothing
        // else (the `uv -> samp.coord` edge is well-typed, so no spurious extras).
        assert_only(&diags, &[(codes::MISSING_OUTPUT, "", None)]);
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
        // EXACTLY the two surplus-Output diagnostics (one per Output node), and no
        // others — both `c -> outN.color` edges are well-typed vec4.
        assert_only(
            &diags,
            &[
                (codes::MULTIPLE_OUTPUTS, "out1", None),
                (codes::MULTIPLE_OUTPUTS, "out2", None),
            ],
        );
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
        // Only the undeclared-LUT error on `lut`; the coord and color edges are
        // well-typed (Sample still infers vec4), so no spurious extras.
        assert_only(&diags, &[(codes::UNKNOWN_TEXTURE, "lut", None)]);
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
        // Only the unresolved-param error on `p`; the Param still infers Float so
        // the downstream construct/output edges resolve clean (no extras).
        assert_only(&diags, &[(codes::UNKNOWN_PARAM, "p", None)]);
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
        // Only the arity error on `mix`; the two wired operands are both vec4 so
        // the operand-type and result-edge checks pass (no spurious extras). The
        // absent third operand is not double-reported as dangling because `mix`'s
        // declared operand list here is just `[x, y]`.
        assert_only(&diags, &[(codes::WRONG_ARITY, "mix", None)]);
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
        // Only the illegal-swizzle error on `sw`; the swizzle's unresolved result
        // type (None) makes the downstream construct/output checks bail rather than
        // cascade, so there are no spurious extras.
        assert_only(&diags, &[(codes::ILLEGAL_SWIZZLE, "sw", None)]);
    }

    // --- dangling required input: Sample.coord unfed -----------------------
    {
        let graph = IrGraph {
            nodes: vec![sample("samp", TextureSource::Source), output("out")],
            edges: vec![IrEdge::new("samp", "out", "out", "color")],
        };
        let diags = check(&graph, &CheckContext::new());
        // Only the dangling-coord error on `samp`; the `samp -> out.color` edge is
        // a well-typed vec4, so no spurious extras.
        assert_only(&diags, &[(codes::DANGLING_INPUT, "samp", Some("coord"))]);
    }

    // --- operand type: add(bool, bool) — Bool is not a float-family arith
    //     operand. The checker must reject it (Float/vecN required) because
    //     lowering+emitter would otherwise produce illegal `bool + bool` GLSL;
    //     a Const(Bool)+Const(Bool)->Output graph type-checks clean today. The
    //     offending OPERAND_TYPE diagnostic must land on the `add` node. ------
    {
        let graph = IrGraph {
            nodes: vec![
                konst("a", ConstValue::Bool { value: true }),
                konst("b", ConstValue::Bool { value: false }),
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
        // Only the bool-operand error on `add`; its result still infers Float
        // (Bool counts as a 1-component numeric in `widest_numeric`) so the
        // downstream construct/output edges resolve clean — no spurious extras.
        assert_only(&diags, &[(codes::OPERAND_TYPE, "add", None)]);
    }

    // --- operand type: mul(vec4, int) — `vector op int` is not legal GLSL
    //     (only `vector op float` broadcasts), so the checker rejects an Int
    //     operand combined with a vector. The OPERAND_TYPE lands on `mul`. ----
    {
        let graph = IrGraph {
            nodes: vec![
                const_vec4("c"),
                konst("i", ConstValue::Int { value: 3 }),
                expr("mul", ExprOp::Mul, &["x", "y"]),
                output("out"),
            ],
            edges: vec![
                IrEdge::new("c", "out", "mul", "x"),
                IrEdge::new("i", "out", "mul", "y"),
                IrEdge::new("mul", "out", "out", "color"),
            ],
        };
        let diags = check(&graph, &CheckContext::new());
        // Only the int-with-vector error on `mul`; the result still infers vec4 so
        // the `mul -> out.color` edge resolves clean — no spurious extras.
        assert_only(&diags, &[(codes::OPERAND_TYPE, "mul", None)]);
    }

    // --- operand type: a scalar Float wired into the fixed-vec2 Sample.coord
    //     port. Broadcasting a scalar into a UV coordinate is almost always a
    //     user error and the lowering/emitter do NOT broadcast it (they emit
    //     `texture(s, <float>)`, illegal GLSL), so the checker tightens the
    //     coord sink to require vec2 — a node-mapped typeMismatch on `samp`. ---
    {
        let graph = IrGraph {
            nodes: vec![
                const_float("uv", 0.5),
                sample("samp", TextureSource::Source),
                output("out"),
            ],
            edges: vec![
                IrEdge::new("uv", "out", "samp", "coord"),
                IrEdge::new("samp", "out", "out", "color"),
            ],
        };
        let diags = check(&graph, &CheckContext::new());
        // Only the float-into-vec2-coord mismatch on `samp`; the coord is fed (no
        // dangling) and `samp -> out.color` is well-typed — no spurious extras.
        assert_only(&diags, &[(codes::TYPE_MISMATCH, "samp", Some("coord"))]);
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
        // The width mismatch on `add` is the primary finding; its result still
        // infers the widest operand (vec3), which then cannot fill the vec4
        // `construct`, so a SECOND operand-type error legitimately lands on `ctor`.
        // Pin BOTH (and nothing else) so the cascade is exact, not loose.
        assert_only(
            &diags,
            &[
                (codes::OPERAND_TYPE, "add", None),
                (codes::OPERAND_TYPE, "ctor", None),
            ],
        );
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
        // Only the unknown-port error on `samp`/`nope`; the bad edge is dropped
        // (never reaches the incoming map) so `samp.coord` is still fed and the
        // color edge is well-typed — no spurious extras.
        assert_only(&diags, &[(codes::UNKNOWN_PORT, "samp", Some("nope"))]);
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
        // Only the unknown-node error on `ghost`; its edge is dropped (source not
        // a real node) so it never duplicates the `c -> out.color` feed and never
        // type-checks — no spurious extras.
        assert_only(&diags, &[(codes::UNKNOWN_NODE, "ghost", None)]);
    }

    // --- CustomSnippet port-type mismatch (#43): a vec2 wired into a snippet
    //     input port declared vec4 is not assignable, and surfaces as a
    //     node-mapped `typeMismatch` on the snippet node + the offending port. ---
    {
        let graph = IrGraph {
            nodes: vec![
                const_vec2("uv"),
                IrNode::new(
                    "snip",
                    NodeOp::CustomSnippet {
                        body: "out_c = in_c;".to_owned(),
                        // Declares a vec4 input port…
                        inputs: vec![PortDecl::new("in_c", PortType::Vec4)],
                        outputs: vec![PortDecl::new("out_c", PortType::Vec4)],
                    },
                ),
                output("out"),
            ],
            edges: vec![
                // …but a vec2 is wired into it (vec2 -> vec4 is neither widen
                // nor broadcast), so the snippet's `in_c` port type-mismatches.
                IrEdge::new("uv", "out", "snip", "in_c"),
                IrEdge::new("snip", "out_c", "out", "color"),
            ],
        };
        let diags = check(&graph, &CheckContext::new());
        // Only the snippet input-port mismatch on `snip`/`in_c`; the snippet's
        // declared vec4 `out_c` still feeds `out.color` cleanly — no spurious
        // extras.
        assert_only(&diags, &[(codes::TYPE_MISMATCH, "snip", Some("in_c"))]);
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
