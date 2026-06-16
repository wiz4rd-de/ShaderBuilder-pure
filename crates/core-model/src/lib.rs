//! `core-model` — the single shared serialization contract.
//!
//! The `Project / Pass / Graph / Node / Parameter` model is defined here once as
//! Rust `serde` types, and the matching TypeScript is **generated** from these
//! types (via `ts-rs`). The *same* schema is used for three things so they can
//! never drift (Architecture §A):
//!
//! 1. **IPC** — Tauri command/event payloads between Rust and the web UI.
//! 2. **The native project file** — JSON on disk (Spec §6).
//! 3. **Import / export** — the in-memory model a `.slangp` maps to.
//!
//! ## Conventions
//!
//! - All fields serialize as **`camelCase`** (`#[serde(rename_all = "camelCase")]`)
//!   so the JSON reads naturally in TypeScript.
//! - Tagged unions use an internal **`"kind"`** discriminator
//!   (`#[serde(tag = "kind")]`), which `ts-rs` turns into a discriminated union.
//! - This is a **skeleton**: the fields are real but minimal. The full node
//!   taxonomy and typed ports arrive with the editor in Phase 5; this only fixes
//!   the shape and the generation pipeline.
//!
//! ## Regenerating the TypeScript bindings
//!
//! ```text
//! cargo test -p core-model        # writes web/src/bindings/*.ts
//! ```
//!
//! `ts-rs`'s `#[ts(export)]` emits a test per type that writes the binding when
//! `cargo test` runs; the output directory is `TS_RS_EXPORT_DIR`, set to
//! `web/src/bindings` in the workspace `.cargo/config.toml`. CI regenerates and
//! fails on any diff, so the committed bindings can never drift from the Rust
//! source of truth.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Crate identity marker. Kept from the Phase 0 scaffold so the inter-crate
/// dependency edges stay referenced; see the workspace stub crates.
pub const NAME: &str = "core-model";

/// Current version of the on-disk project schema. Bump on any breaking change so
/// old project files can be detected and migrated.
pub const PROJECT_SCHEMA_VERSION: u32 = 1;

/// A ShaderBuilder project — the native project file (Spec §6). Distinct from an
/// exported `.slangp` bundle; this is the editable document the frontend owns
/// (Architecture §A).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Project {
    /// Schema version of this project file; see [`PROJECT_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Human-readable project name.
    pub name: String,
    /// The render pipeline: an ordered list of passes. Maps 1:1 to a `.slangp`
    /// (Spec §3, pipeline view).
    pub passes: Vec<Pass>,
}

/// One render pass — exactly one fragment shader (Spec §3). A pass is authored
/// either as a node [`Graph`] or supplied as opaque whole-pass code (e.g. from
/// preset import).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Pass {
    /// Stable unique id (used by the pipeline view and to map diagnostics back).
    pub id: String,
    /// Display name / alias for the pass.
    pub name: String,
    /// What produces this pass's fragment shader.
    pub source: PassSource,
    /// `#pragma parameter` knobs this pass exposes as live sliders (Spec §4).
    pub parameters: Vec<Parameter>,
}

/// How a pass's shader is defined (Spec §3, Architecture §C).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum PassSource {
    /// A node graph that lowers to one fragment shader.
    Graph {
        /// The per-pass node graph.
        graph: Graph,
    },
    /// Opaque whole-pass slang source taken verbatim — the escape hatch, and
    /// what preset import produces (Spec §3/§5).
    WholePassCode {
        /// The complete `.slang` pass source.
        source: String,
    },
}

/// A per-pass node graph — a typed dataflow DAG (Architecture §C). Skeletal
/// here; the node taxonomy and typed ports arrive with the editor in Phase 5.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Graph {
    /// The nodes in this graph.
    pub nodes: Vec<Node>,
    /// The directed, port-to-port connections between nodes.
    pub edges: Vec<Edge>,
}

/// A single node in a per-pass [`Graph`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Node {
    /// Stable unique id (referenced by [`Edge`]s and diagnostics).
    pub id: String,
    /// Node type key, e.g. `"sampler.source"`, `"math.mix"`, `"output"`. The
    /// full taxonomy is defined in Phase 5 (Spec §8.3).
    pub kind: String,
    /// Position on the editor canvas.
    pub position: Vec2,
    /// Node configuration. Free-form JSON until the typed node set lands; the
    /// generated TypeScript types this as `Record<string, unknown>`.
    #[serde(default)]
    #[ts(type = "Record<string, unknown>")]
    pub data: serde_json::Value,
}

/// A directed connection between two node ports (Architecture §C, Spec §8.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Edge {
    /// Stable unique id for this connection.
    pub id: String,
    /// Id of the node the connection starts at.
    pub source: String,
    /// Output port on the source node.
    pub source_port: String,
    /// Id of the node the connection ends at.
    pub target: String,
    /// Input port on the target node.
    pub target_port: String,
}

/// A `#pragma parameter` declaration — a runtime knob shown as a slider (Spec §4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Parameter {
    /// Identifier used in the shader (the `#pragma parameter` name).
    pub name: String,
    /// Human-readable label shown in the slider UI.
    pub label: String,
    /// Default value.
    pub default: f32,
    /// Minimum slider value.
    pub min: f32,
    /// Maximum slider value.
    pub max: f32,
    /// Slider increment.
    pub step: f32,
}

/// A 2D vector, used for editor node positions.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Project {
    /// A new, empty project at the current schema version.
    pub fn empty(name: impl Into<String>) -> Self {
        Self {
            schema_version: PROJECT_SCHEMA_VERSION,
            name: name.into(),
            passes: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_marker() {
        assert_eq!(NAME, "core-model");
    }

    #[test]
    fn project_json_round_trips() {
        let project = Project {
            schema_version: PROJECT_SCHEMA_VERSION,
            name: "Demo".to_owned(),
            passes: vec![
                Pass {
                    id: "pass-0".to_owned(),
                    name: "CRT".to_owned(),
                    source: PassSource::Graph {
                        graph: Graph {
                            nodes: vec![Node {
                                id: "n0".to_owned(),
                                kind: "output".to_owned(),
                                position: Vec2 { x: 1.0, y: 2.0 },
                                data: serde_json::json!({ "note": "skeleton" }),
                            }],
                            edges: vec![],
                        },
                    },
                    parameters: vec![Parameter {
                        name: "BRIGHTNESS".to_owned(),
                        label: "Brightness".to_owned(),
                        default: 1.0,
                        min: 0.0,
                        max: 2.0,
                        step: 0.01,
                    }],
                },
                Pass {
                    id: "pass-1".to_owned(),
                    name: "Imported".to_owned(),
                    source: PassSource::WholePassCode {
                        source: "// verbatim slang".to_owned(),
                    },
                    parameters: vec![],
                },
            ],
        };

        let json = serde_json::to_string_pretty(&project).expect("serialize");
        let back: Project = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(project, back);
    }

    #[test]
    fn pass_source_is_internally_tagged() {
        let graph = PassSource::Graph {
            graph: Graph::default(),
        };
        let value = serde_json::to_value(&graph).unwrap();
        assert_eq!(value["kind"], "graph");

        let code = PassSource::WholePassCode {
            source: "x".to_owned(),
        };
        let value = serde_json::to_value(&code).unwrap();
        assert_eq!(value["kind"], "wholePassCode");
        assert_eq!(value["source"], "x");
    }
}
