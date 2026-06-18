//! Pre-export VALIDATION gate (#64): a fail-closed check that a
//! [`core_model::Project`] can be written as a RetroArch `.slangp` bundle, run
//! BEFORE the writer ([`crate::export_preset`]) is invoked.
//!
//! ## Why a separate gate (Spec §8 item 4)
//!
//! An invalid graph must never be silently exported. The frontend already
//! substitutes each compiled graph pass with its generated whole-pass slang
//! (`exportSubstitution.ts`) and refuses when the pipeline is invalid, but the
//! Rust side adds a defense-in-depth gate so the writer is never asked to
//! serialize a project it cannot represent. The gate returns STRUCTURED reasons
//! ([`core_model::ExportBlocker`]) the UX lists as the blocking reasons — it does
//! not throw, so the caller can render every blocker at once.
//!
//! The blockers are intentionally a superset of the writer's
//! [`crate::ExportError`]: an `UncompiledGraphPass` blocker maps to the writer's
//! `GraphPassUnsupported` (which should therefore NEVER fire post-substitution —
//! if it does, it is an internal error), and the gate catches two further cases
//! the writer would otherwise produce a broken bundle for (no passes, empty pass
//! body).

use core_model::{ExportBlocker, ExportValidation, PassSource, Project};

/// Validate `project` for export, returning the structured blockers (#64). An
/// empty blocker list ([`ExportValidation::ok`]) means the project is exportable;
/// the export UX must refuse to invoke [`crate::export_preset`] otherwise.
///
/// Checks, in order:
/// 1. the project has at least one pass;
/// 2. no pass is still an unresolved node [`core_model::Graph`] (the writer cannot
///    serialize one — graph passes must be substituted with their generated slang
///    before export, which only succeeds for a compiled pass);
/// 3. no whole-pass code pass has an empty source body.
pub fn validate_for_export(project: &Project) -> ExportValidation {
    let mut blockers = Vec::new();

    if project.passes.is_empty() {
        blockers.push(ExportBlocker::NoPasses);
    }

    for pass in &project.passes {
        match &pass.source {
            PassSource::Graph { .. } => {
                blockers.push(ExportBlocker::UncompiledGraphPass {
                    pass_id: pass.id.clone(),
                    pass_name: pass.name.clone(),
                });
            }
            PassSource::WholePassCode { source, .. } => {
                if source.trim().is_empty() {
                    blockers.push(ExportBlocker::EmptyPassSource {
                        pass_id: pass.id.clone(),
                        pass_name: pass.name.clone(),
                    });
                }
            }
        }
    }

    ExportValidation { blockers }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::{Graph, Pass, PassSettings, Project};

    fn wpc_pass(id: &str, source: &str) -> Pass {
        Pass {
            id: id.to_owned(),
            name: id.to_owned(),
            source: PassSource::WholePassCode {
                source: source.to_owned(),
                filename: None,
                opaque: true,
            },
            parameters: vec![],
            references: vec![],
            settings: PassSettings::default(),
        }
    }

    fn graph_pass(id: &str) -> Pass {
        Pass {
            id: id.to_owned(),
            name: id.to_owned(),
            source: PassSource::Graph {
                graph: Graph::default(),
            },
            parameters: vec![],
            references: vec![],
            settings: PassSettings::default(),
        }
    }

    #[test]
    fn a_whole_pass_project_is_exportable() {
        let project = Project {
            passes: vec![wpc_pass("p0", "#version 450\nvoid main(){}\n")],
            ..Project::empty("ok")
        };
        let v = validate_for_export(&project);
        assert!(v.ok(), "blockers: {:?}", v.blockers);
    }

    #[test]
    fn an_empty_project_is_blocked() {
        let project = Project::empty("empty");
        let v = validate_for_export(&project);
        assert!(!v.ok());
        assert!(matches!(v.blockers.as_slice(), [ExportBlocker::NoPasses]));
    }

    #[test]
    fn a_graph_pass_blocks_export_with_its_id() {
        let project = Project {
            passes: vec![graph_pass("g0")],
            ..Project::empty("graph")
        };
        let v = validate_for_export(&project);
        assert!(!v.ok());
        match v.blockers.as_slice() {
            [ExportBlocker::UncompiledGraphPass { pass_id, .. }] => assert_eq!(pass_id, "g0"),
            other => panic!("expected one UncompiledGraphPass, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_source_pass_is_blocked() {
        let project = Project {
            passes: vec![wpc_pass("blank", "   \n\t")],
            ..Project::empty("blank")
        };
        let v = validate_for_export(&project);
        assert!(!v.ok());
        match v.blockers.as_slice() {
            [ExportBlocker::EmptyPassSource { pass_id, .. }] => assert_eq!(pass_id, "blank"),
            other => panic!("expected one EmptyPassSource, got {other:?}"),
        }
    }

    #[test]
    fn multiple_blockers_are_all_reported() {
        let project = Project {
            passes: vec![graph_pass("g0"), wpc_pass("blank", "")],
            ..Project::empty("mixed")
        };
        let v = validate_for_export(&project);
        assert_eq!(v.blockers.len(), 2);
    }

    /// #64 acceptance SMOKE TEST: a VALID, editor-style project (all whole-pass
    /// after substitution, with a LUT + a tuned parameter) passes the gate, exports
    /// to a RetroArch-conventional bundle, and re-imports LOSSLESSLY (reusing the
    /// Phase-3 export → re-import round trip). Proves "exporting a valid preset
    /// writes a directory that re-imports losslessly + matches the RA convention".
    #[test]
    fn valid_project_passes_gate_and_re_imports_losslessly() {
        use core_model::{Lut, Parameter, PassSettings, WrapMode};

        // Build a LUT source PNG on disk for the exporter to copy in.
        let src = tempfile::tempdir().unwrap();
        let lut_path = src.path().join("border.png");
        std::fs::write(&lut_path, b"\x89PNG\r\n\x1a\nfake").unwrap();

        let project = Project {
            passes: vec![Pass {
                id: "p0".to_owned(),
                name: "Pass".to_owned(),
                source: PassSource::WholePassCode {
                    source: "#version 450\n\
                             #pragma parameter BRIGHT \"Brightness\" 1.0 0.0 2.0 0.01\n\
                             void main(){}\n"
                        .to_owned(),
                    filename: Some("pass0.slang".to_owned()),
                    opaque: true,
                },
                parameters: vec![],
                references: vec![],
                settings: PassSettings::default(),
            }],
            parameters: vec![Parameter {
                name: "BRIGHT".to_owned(),
                label: "Brightness".to_owned(),
                // A tuned default that differs from the pragma initial (1.0) must be
                // emitted inline so it survives the round trip.
                default: 1.5,
                min: 0.0,
                max: 2.0,
                step: 0.01,
            }],
            luts: vec![Lut {
                name: "BORDER".to_owned(),
                path: lut_path.to_string_lossy().into_owned(),
                filter_linear: Some(true),
                wrap_mode: Some(WrapMode::ClampToEdge),
                mipmap: Some(false),
            }],
            ..Project::empty("Smoke")
        };

        // 1. The gate accepts the valid project.
        assert!(
            validate_for_export(&project).ok(),
            "a valid whole-pass project must pass the export gate"
        );

        // 2. Export the bundle.
        let out = tempfile::tempdir().unwrap();
        let report = crate::export_preset(&project, out.path(), &std::collections::BTreeMap::new())
            .expect("export");

        // 3. RA convention: relative paths, per-pass .slang, LUT under textures/,
        //    inline param default.
        let preset_path = out.path().join(crate::PRESET_FILENAME);
        assert!(preset_path.is_file(), "preset.slangp written");
        assert!(out.path().join(&report.pass_files[0]).is_file(), ".slang written");
        assert_eq!(report.texture_files.len(), 1);
        assert!(out
            .path()
            .join(crate::TEXTURES_DIR)
            .join(&report.texture_files[0])
            .is_file());
        let text = std::fs::read_to_string(&preset_path).unwrap();
        for line in text.lines() {
            let value = line.split_once('=').map(|(_, v)| v.trim()).unwrap_or("");
            assert!(!value.starts_with('/'), "relative paths only: {line:?}");
        }
        assert!(text.contains("BRIGHT = 1.5"), "tuned default inline:\n{text}");
        assert!(text.contains("textures/"), "LUT referenced relatively:\n{text}");

        // 4. Re-import the exported bundle (the Phase-3 round trip) — it parses and
        //    preserves the passes, the tuned parameter, and the LUT.
        let (reimported, _diags) = crate::import_preset(&preset_path).expect("re-import");
        assert_eq!(reimported.passes.len(), 1);
        let bright = reimported
            .parameters
            .iter()
            .find(|p| p.name == "BRIGHT")
            .expect("tuned parameter survives the round trip");
        assert_eq!(bright.default, 1.5);
        assert!(
            reimported.luts.iter().any(|l| l.name == "BORDER"),
            "LUT survives the round trip"
        );
    }
}
