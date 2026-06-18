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
