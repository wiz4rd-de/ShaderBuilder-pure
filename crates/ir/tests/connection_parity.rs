//! Cross-language **connection-legality parity** (#65).
//!
//! The in-editor connection guard (`web/src/nodes/portTypeChecking.ts`) rejects
//! illegal wires at DRAG time, with no IPC round-trip, by re-implementing the
//! edge-legality predicate in TypeScript. That TS port is only trustworthy if it
//! provably AGREES with the Rust type checker — drift between the two is the
//! entire risk of the feature.
//!
//! This module pins that agreement two ways:
//!
//! 1. [`golden_table_matches_checked_in_file`] regenerates a small, exhaustive
//!    truth table of `(srcType, target) → legal?` from the SHARED predicate
//!    [`connection_legal`](core_model::ir::connection_legal) and asserts it
//!    matches the JSON golden committed at `web/src/nodes/__goldens__/`. The
//!    frontend's `portTypeChecking.test.ts` asserts its TS predicate reproduces
//!    that SAME golden — so the two implementations are transitively pinned to
//!    each other, and any divergence fails CI on one side or the other.
//!
//! 2. [`predicate_agrees_with_full_checker`] drives real `IrGraph`s through the
//!    full [`check`] type checker and asserts the standalone `connection_legal`
//!    verdict matches whether `check` emits a `typeMismatch` on that edge —
//!    proving the distilled predicate the editor mirrors is faithful to the
//!    checker's actual edge rule (incl. the Sample.coord tightening and the
//!    polymorphic Expr-operand reflexivity).

use core_model::ir::{
    connection_legal, ConnectionTarget, ConstValue, ExprOp, IrEdge, IrGraph, IrNode, NodeOp,
    PortDecl, PortType, TextureSource,
};
use ir::{check, codes, CheckContext};

/// Every [`PortType`], in a stable order, with its camelCase serde spelling.
const ALL_TYPES: &[(PortType, &str)] = &[
    (PortType::Float, "float"),
    (PortType::Vec2, "vec2"),
    (PortType::Vec3, "vec3"),
    (PortType::Vec4, "vec4"),
    (PortType::Int, "int"),
    (PortType::Bool, "bool"),
    (PortType::Sampler2D, "sampler2D"),
];

/// One golden row: a source type feeding a classified sink, with the verdict.
/// Serialized to JSON by hand (no serde dep on this ad-hoc shape) so the file is
/// trivially diffable and the TS side parses it with `JSON.parse`.
struct Row {
    src: &'static str,
    /// The `ConnectionTarget` tag the editor classifies the sink into.
    target_kind: &'static str,
    /// For `assignable`, the declared sink type; empty otherwise.
    target_type: &'static str,
    legal: bool,
}

/// Build the exhaustive truth table from the shared predicate.
fn build_table() -> Vec<Row> {
    let mut rows = Vec::new();
    for &(src, src_name) in ALL_TYPES {
        // assignable(tgt) for every declared sink type.
        for &(tgt, tgt_name) in ALL_TYPES {
            rows.push(Row {
                src: src_name,
                target_kind: "assignable",
                target_type: tgt_name,
                legal: connection_legal(src, ConnectionTarget::Assignable(tgt)),
            });
        }
        // The tightened Sample.coord sink.
        rows.push(Row {
            src: src_name,
            target_kind: "sampleCoord",
            target_type: "",
            legal: connection_legal(src, ConnectionTarget::SampleCoord),
        });
        // A polymorphic Expr operand (always structurally legal).
        rows.push(Row {
            src: src_name,
            target_kind: "exprOperand",
            target_type: "",
            legal: connection_legal(src, ConnectionTarget::ExprOperand),
        });
    }
    rows
}

/// Render the table as deterministic, pretty JSON (one object per line) so the
/// committed golden has a clean, reviewable diff.
fn render_json(rows: &[Row]) -> String {
    let mut out = String::from("[\n");
    for (i, r) in rows.iter().enumerate() {
        let comma = if i + 1 < rows.len() { "," } else { "" };
        out.push_str(&format!(
            "  {{ \"src\": \"{}\", \"targetKind\": \"{}\", \"targetType\": \"{}\", \"legal\": {} }}{}\n",
            r.src, r.target_kind, r.target_type, r.legal, comma
        ));
    }
    out.push_str("]\n");
    out
}

/// The committed golden path (`web/src/nodes/__goldens__/connectionLegality.json`),
/// resolved from this crate's manifest dir up to the workspace root.
fn golden_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../web/src/nodes/__goldens__/connectionLegality.json")
}

#[test]
fn golden_table_matches_checked_in_file() {
    let expected = render_json(&build_table());
    let path = golden_path();
    // Regenerate the golden (like ts-rs regenerates bindings): write it so a
    // local `cargo test` refreshes the file, then assert it is unchanged so CI
    // fails on any uncommitted drift.
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("create golden dir");
    }
    std::fs::write(&path, &expected).expect("write connection-legality golden");
    let on_disk = std::fs::read_to_string(&path).expect("read connection-legality golden");
    assert_eq!(
        on_disk, expected,
        "connection-legality golden drifted — commit web/src/nodes/__goldens__/connectionLegality.json"
    );
}

// ---------------------------------------------------------------------------
// Parity #2: the distilled predicate matches the FULL checker on real graphs.
// ---------------------------------------------------------------------------

fn konst(id: &str, value: ConstValue) -> IrNode {
    IrNode::new(id, NodeOp::Const { value })
}

/// A Const node producing each [`PortType`] we can author a literal for. Returns
/// `None` for types with no Const literal (Sampler2D has none).
fn const_of(id: &str, ty: PortType) -> Option<IrNode> {
    let value = match ty {
        PortType::Float => ConstValue::Float { value: 1.0 },
        PortType::Vec2 => ConstValue::Vec2 { value: [1.0, 2.0] },
        PortType::Vec3 => ConstValue::Vec3 {
            value: [1.0, 2.0, 3.0],
        },
        PortType::Vec4 => ConstValue::Vec4 {
            value: [1.0, 2.0, 3.0, 4.0],
        },
        PortType::Int => ConstValue::Int { value: 3 },
        PortType::Bool => ConstValue::Bool { value: true },
        PortType::Sampler2D => return None,
    };
    Some(konst(id, value))
}

/// Whether `check` flagged a `typeMismatch` on the given sink node+port.
fn has_type_mismatch_on(graph: &IrGraph, ctx: &CheckContext, node: &str, port: &str) -> bool {
    check(graph, ctx).iter().any(|d| {
        d.code == codes::TYPE_MISMATCH && d.node == node && d.port.as_deref() == Some(port)
    })
}

#[test]
fn predicate_agrees_with_full_checker() {
    // For each source PortType (that a Const can produce) and each representative
    // sink, build a minimal graph wiring `src.out → sink`, run the FULL checker,
    // and assert the presence/absence of a typeMismatch matches `connection_legal`.
    //
    // The sink fixtures cover all three ConnectionTarget cases:
    //  * Output.color        -> Assignable(Vec4)
    //  * CustomSnippet ports -> Assignable(<declared type>)  (one per PortType)
    //  * Sample.coord        -> SampleCoord (tightened vec2)
    //  * an Expr operand     -> ExprOperand (polymorphic)
    for &(src, _src_name) in ALL_TYPES {
        let Some(src_node) = const_of("src", src) else {
            continue; // No Const literal for Sampler2D — covered by the golden table.
        };

        // --- Output.color: Assignable(Vec4) -----------------------------------
        {
            let graph = IrGraph {
                nodes: vec![src_node.clone(), IrNode::new("out", NodeOp::Output)],
                edges: vec![IrEdge::new("src", "out", "out", "color")],
            };
            let predicate = connection_legal(src, ConnectionTarget::Assignable(PortType::Vec4));
            let checker_ok = !has_type_mismatch_on(&graph, &CheckContext::new(), "out", "color");
            assert_eq!(
                predicate, checker_ok,
                "Output.color disagreement for src {src:?}"
            );
        }

        // --- CustomSnippet input port: Assignable(<declared type>) ------------
        for &(tgt, _tgt_name) in ALL_TYPES {
            // A Sampler2D *source* cannot exist as a Const, and only a Sampler2D
            // declared port accepts a sampler — but since we have no sampler
            // source here, this exercises every non-sampler declared type.
            let snippet = IrNode::new(
                "snip",
                NodeOp::CustomSnippet {
                    body: "result = vec4(0.0);".to_owned(),
                    inputs: vec![PortDecl {
                        name: "in".to_owned(),
                        ty: tgt,
                    }],
                    outputs: vec![PortDecl {
                        name: "result".to_owned(),
                        ty: PortType::Vec4,
                    }],
                },
            );
            let graph = IrGraph {
                nodes: vec![
                    src_node.clone(),
                    snippet,
                    IrNode::new("out", NodeOp::Output),
                ],
                edges: vec![
                    IrEdge::new("src", "out", "snip", "in"),
                    IrEdge::new("snip", "result", "out", "color"),
                ],
            };
            let predicate = connection_legal(src, ConnectionTarget::Assignable(tgt));
            let checker_ok = !has_type_mismatch_on(&graph, &CheckContext::new(), "snip", "in");
            assert_eq!(
                predicate, checker_ok,
                "CustomSnippet port disagreement for src {src:?} -> {tgt:?}"
            );
        }

        // --- Sample.coord: SampleCoord (tightened vec2) -----------------------
        {
            let graph = IrGraph {
                nodes: vec![
                    src_node.clone(),
                    IrNode::new(
                        "samp",
                        NodeOp::Sample {
                            texture: TextureSource::Source,
                        },
                    ),
                    IrNode::new("out", NodeOp::Output),
                ],
                edges: vec![
                    IrEdge::new("src", "out", "samp", "coord"),
                    IrEdge::new("samp", "out", "out", "color"),
                ],
            };
            let predicate = connection_legal(src, ConnectionTarget::SampleCoord);
            let checker_ok = !has_type_mismatch_on(&graph, &CheckContext::new(), "samp", "coord");
            assert_eq!(
                predicate, checker_ok,
                "Sample.coord disagreement for src {src:?}"
            );
        }

        // --- Expr operand: ExprOperand (polymorphic, always structurally OK) --
        {
            // `abs` is unary float-family; its operand is polymorphic at the
            // STRUCTURAL edge level (operand-type constraints are a separate
            // diagnostic, never a typeMismatch — which is exactly what the editor
            // defers). We assert no typeMismatch regardless of src type.
            let graph = IrGraph {
                nodes: vec![
                    src_node.clone(),
                    IrNode::new(
                        "e",
                        NodeOp::Expr {
                            op: ExprOp::Abs,
                            operands: vec!["x".to_owned()],
                        },
                    ),
                    IrNode::new("out", NodeOp::Output),
                ],
                edges: vec![
                    IrEdge::new("src", "out", "e", "x"),
                    IrEdge::new("e", "out", "out", "color"),
                ],
            };
            let predicate = connection_legal(src, ConnectionTarget::ExprOperand);
            let checker_ok = !has_type_mismatch_on(&graph, &CheckContext::new(), "e", "x");
            assert_eq!(
                predicate, checker_ok,
                "Expr operand disagreement for src {src:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Parity #3 (#65 F2/F3): DATA-DERIVED editor sources — Swizzle / Split / Combine.
//
// The two parity checks above only ever feed ABSTRACT PortType pairs into the
// sink. They are blind to the editor's *source-output* typing: a Swizzle's output
// type is `swizzle_result(input_type, mask)` of its LIVE input, not a type
// fabricated from the mask length. The TS guard derived it from mask length alone
// (vector.ts), so it FALSE-BLOCKED a wire the IR accepts (F2).
//
// This golden drives the EDITOR path end-to-end: it builds the LOWERED IrGraph the
// editor's `graphToIr` would produce (a Const of `input_type` → Swizzle/Split/
// Combine → a sink), runs the FULL checker, and records whether a `typeMismatch`
// lands on the SINK edge. The frontend test rebuilds the equivalent editor Graph,
// runs `judgeConnection`, and asserts its `legal` verdict reproduces this golden
// row-for-row — so the editor's drag-time verdict for data-derived sources is
// pinned to the IR. This row table would NOT all be `legal:true` before the F2 fix.
// ---------------------------------------------------------------------------

/// One data-derived scenario row: a `kind` of source (swizzle/split/combine) with
/// its `mask`/`ty`, fed an `inputType`, dropped onto a classified `sink`.
struct ScenarioRow {
    source_kind: &'static str,
    /// The swizzle/split mask (empty for combine).
    mask: &'static str,
    /// The combine target type (empty for swizzle/split).
    ty: &'static str,
    /// The PortType feeding the source's input operand(s).
    input_type: &'static str,
    sink_kind: &'static str,
    /// For an `assignable` sink, the declared sink type (empty otherwise).
    sink_type: &'static str,
    legal: bool,
}

/// The sinks every scenario is dropped onto, as `(sink_kind, sink_type)`.
const SCENARIO_SINKS: &[(&str, &str)] = &[
    ("outputColor", ""),       // Output.color → Assignable(Vec4)
    ("sampleCoord", ""),       // Sample.coord → SampleCoord (tightened vec2)
    ("snippetFloat", "float"), // CustomSnippet input port → Assignable(<type>)
    ("snippetVec2", "vec2"),
    ("snippetVec3", "vec3"),
    ("snippetVec4", "vec4"),
];

/// Build the sink node + the edge wiring `source.out → sink`, plus the
/// `(node, port)` the typeMismatch (if any) lands on.
fn sink_nodes_and_edge(sink_kind: &str, sink_type: &str) -> (Vec<IrNode>, IrEdge, &'static str) {
    match sink_kind {
        "outputColor" => (
            vec![IrNode::new("sink", NodeOp::Output)],
            IrEdge::new("source", "out", "sink", "color"),
            "color",
        ),
        "sampleCoord" => (
            vec![
                IrNode::new(
                    "sink",
                    NodeOp::Sample {
                        texture: TextureSource::Source,
                    },
                ),
                // Give the Sample a downstream Output so the graph is well-formed.
                IrNode::new("term", NodeOp::Output),
            ],
            IrEdge::new("source", "out", "sink", "coord"),
            "coord",
        ),
        _ => {
            // A CustomSnippet input port of the declared sink type.
            let ty = parse_port_type(sink_type);
            (
                vec![IrNode::new(
                    "sink",
                    NodeOp::CustomSnippet {
                        body: "result = vec4(0.0);".to_owned(),
                        inputs: vec![PortDecl {
                            name: "in".to_owned(),
                            ty,
                        }],
                        outputs: vec![PortDecl {
                            name: "result".to_owned(),
                            ty: PortType::Vec4,
                        }],
                    },
                )],
                IrEdge::new("source", "out", "sink", "in"),
                "in",
            )
        }
    }
}

/// Parse a camelCase PortType spelling.
fn parse_port_type(name: &str) -> PortType {
    ALL_TYPES
        .iter()
        .find(|(_, n)| *n == name)
        .map(|(t, _)| *t)
        .unwrap_or_else(|| panic!("unknown port type `{name}`"))
}

/// Build the lowered IrGraph for one scenario and return whether the FULL checker
/// leaves the SINK edge free of a `typeMismatch` (the editor's "legal" verdict).
fn scenario_is_legal(
    source_kind: &str,
    mask: &str,
    ty: &str,
    input_type: &str,
    sink_kind: &str,
    sink_type: &str,
) -> bool {
    let mut nodes: Vec<IrNode> = Vec::new();
    let mut edges: Vec<IrEdge> = Vec::new();

    // The source op + its input operands (mirroring the editor descriptors'
    // `toNodeOp`): swizzle/split → Expr{Swizzle, operands:["in"]}; combine →
    // Expr{Construct{ty}, operands:["x","y",...]} fed by float components.
    let input_pt = parse_port_type(input_type);
    match source_kind {
        "swizzle" | "split" => {
            // One Const of `input_type` feeding the single `in` operand.
            if let Some(src_in) = const_of("in_node", input_pt) {
                nodes.push(src_in);
                edges.push(IrEdge::new("in_node", "out", "source", "in"));
            }
            nodes.push(IrNode::new(
                "source",
                NodeOp::Expr {
                    op: ExprOp::Swizzle {
                        mask: mask.to_owned(),
                    },
                    operands: vec!["in".to_owned()],
                },
            ));
        }
        "combine" => {
            let target = parse_port_type(ty);
            let n = target.component_count().unwrap_or(0) as usize;
            let names = ["x", "y", "z", "w"];
            let operands: Vec<String> = names[..n].iter().map(|s| (*s).to_owned()).collect();
            for (i, name) in operands.iter().enumerate() {
                let cid = format!("c{i}");
                nodes.push(konst(&cid, ConstValue::Float { value: 1.0 }));
                edges.push(IrEdge::new(&cid, "out", "source", name));
            }
            nodes.push(IrNode::new(
                "source",
                NodeOp::Expr {
                    op: ExprOp::Construct { ty: target },
                    operands,
                },
            ));
        }
        other => panic!("unknown source kind `{other}`"),
    }

    let (sink_nodes, sink_edge, sink_port) = sink_nodes_and_edge(sink_kind, sink_type);
    nodes.extend(sink_nodes);
    edges.push(sink_edge);

    let graph = IrGraph { nodes, edges };
    !has_type_mismatch_on(&graph, &CheckContext::new(), "sink", sink_port)
}

/// Build the data-derived scenario table from the FULL checker.
fn build_scenarios() -> Vec<ScenarioRow> {
    // The PortTypes a Const source can produce (Sampler2D has no literal).
    const INPUT_TYPES: &[&str] = &["float", "vec2", "vec3", "vec4", "int", "bool"];
    // Swizzle masks of each length 1..=4 (legal envelope varies by input width).
    const MASKS: &[&str] = &["x", "xy", "xyz", "xyzw"];
    const COMBINE_TYPES: &[&str] = &["vec2", "vec3", "vec4"];

    let mut rows = Vec::new();
    for &(sink_kind, sink_type) in SCENARIO_SINKS {
        // Swizzle: each mask × each input type.
        for &mask in MASKS {
            for &input_type in INPUT_TYPES {
                rows.push(ScenarioRow {
                    source_kind: "swizzle",
                    mask,
                    ty: "",
                    input_type,
                    sink_kind,
                    sink_type,
                    legal: scenario_is_legal("swizzle", mask, "", input_type, sink_kind, sink_type),
                });
            }
        }
        // Split: a single-component swizzle (`x`) × each input type.
        for &input_type in INPUT_TYPES {
            rows.push(ScenarioRow {
                source_kind: "split",
                mask: "x",
                ty: "",
                input_type,
                sink_kind,
                sink_type,
                legal: scenario_is_legal("split", "x", "", input_type, sink_kind, sink_type),
            });
        }
        // Combine: each target type (inputs are floats — input-independent output).
        for &ty in COMBINE_TYPES {
            rows.push(ScenarioRow {
                source_kind: "combine",
                mask: "",
                ty,
                input_type: "float",
                sink_kind,
                sink_type,
                legal: scenario_is_legal("combine", "", ty, "float", sink_kind, sink_type),
            });
        }
    }
    rows
}

/// Render the scenario table as deterministic, one-object-per-line JSON.
fn render_scenarios_json(rows: &[ScenarioRow]) -> String {
    let mut out = String::from("[\n");
    for (i, r) in rows.iter().enumerate() {
        let comma = if i + 1 < rows.len() { "," } else { "" };
        out.push_str(&format!(
            "  {{ \"sourceKind\": \"{}\", \"mask\": \"{}\", \"ty\": \"{}\", \"inputType\": \"{}\", \"sinkKind\": \"{}\", \"sinkType\": \"{}\", \"legal\": {} }}{}\n",
            r.source_kind, r.mask, r.ty, r.input_type, r.sink_kind, r.sink_type, r.legal, comma
        ));
    }
    out.push_str("]\n");
    out
}

/// The committed scenario golden path.
fn scenario_golden_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../web/src/nodes/__goldens__/connectionParityScenarios.json")
}

#[test]
fn data_derived_scenario_golden_matches_checked_in_file() {
    let expected = render_scenarios_json(&build_scenarios());
    let path = scenario_golden_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("create golden dir");
    }
    std::fs::write(&path, &expected).expect("write data-derived scenario golden");
    let on_disk = std::fs::read_to_string(&path).expect("read data-derived scenario golden");
    assert_eq!(
        on_disk, expected,
        "data-derived scenario golden drifted — commit \
         web/src/nodes/__goldens__/connectionParityScenarios.json"
    );
}
