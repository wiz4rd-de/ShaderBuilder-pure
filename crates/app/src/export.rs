//! The `export_preset` Tauri command (#36): write a frontend-owned
//! [`core_model::Project`] out as a RetroArch-conventional `.slangp` bundle.
//!
//! The heavy lifting — directory layout, byte-exact pass `.slang` files, LUT
//! copying, relative paths, inline parameter defaults, preserved-key re-emission —
//! lives in [`preset_io::export_preset`]. This module is the thin IPC seam: it
//! takes the project the editor holds plus a destination directory, calls the
//! writer, and returns the typed [`core_model::ExportResult`] the UI can show — or
//! the typed [`core_model::ExportError`] on failure.
//!
//! ## Typed surface (single shared schema, Fix C1)
//!
//! Both the success ([`core_model::ExportResult`]) and error
//! ([`core_model::ExportError`]) payloads live in `core-model`, so TypeScript
//! bindings are generated from the one shared schema (core-model module doc §A)
//! instead of escaping it as an untyped string. Mirroring the
//! `save_project`/`load_project` precedent ([`crate::project`]), the error keeps
//! its two failure modes — a write `Io` failure and the expected
//! `GraphPassUnsupported` limitation — as **branchable** variants. The internal
//! [`preset_io::ExportError`] is mapped at this seam by [`to_typed_error`].
//!
//! ## Extras (preserved unknown keys)
//!
//! [`preset_io::export_preset`] re-emits the unknown keys the parser preserved on
//! import (the #33 `extras` map). The editable [`core_model::Project`] does not
//! (yet) carry that map, so this command passes an **empty** extras set: a project
//! exported straight from the editor has no preserved unknown keys to round-trip.
//! The lossless import → export round trip with extras is covered at the
//! `preset-io` layer, where both the project and its extras are in hand.

use std::collections::BTreeMap;

use core_model::{ExportError, ExportResult, ExportValidation};

/// Map the crate-internal [`preset_io::ExportError`] into the webview-facing,
/// TS-exported [`core_model::ExportError`] (Fix C1). The two failure modes stay
/// **distinct** so the frontend can branch — the expected `GraphPassUnsupported`
/// limitation never collapses into the same opaque string as a real write error.
/// `std::io::Error` is flattened to a message rather than leaked across IPC.
fn to_typed_error(err: preset_io::ExportError) -> ExportError {
    match err {
        preset_io::ExportError::Io(e) => ExportError::Io {
            message: e.to_string(),
        },
        preset_io::ExportError::GraphPassUnsupported(pass_id) => {
            ExportError::GraphPassUnsupported { pass_id }
        }
    }
}

/// Export the editor's current [`core_model::Project`] as a RetroArch bundle under
/// `dest_dir` (#36). Writes `preset.slangp` + per-pass `.slang` + `textures/` LUT
/// PNGs with **relative** paths and inline parameter defaults; returns a typed
/// [`ExportResult`] summary. On failure returns the typed [`ExportError`] — a
/// write failure (`Io`) or the expected graph-pass limitation
/// (`GraphPassUnsupported`) — so the webview can branch on the variant rather than
/// parse a string.
#[tauri::command]
pub fn export_preset(
    project: core_model::Project,
    dest_dir: String,
) -> Result<ExportResult, ExportError> {
    // Fail-closed gate (#64, Spec §8 item 4): an invalid project is NEVER silently
    // exported. The frontend runs `validate_export` first and disables the button,
    // but the command re-checks as defense-in-depth so the writer is never invoked
    // on a project it cannot represent. A blocker maps to the matching typed
    // `ExportError` (a graph pass → `GraphPassUnsupported`, the very error the
    // writer would otherwise raise — surfaced HERE before any bytes are written).
    let validation = preset_io::validate_for_export(&project);
    if let Some(err) = blocker_to_error(&validation) {
        return Err(err);
    }
    // The editable project carries no preserved unknown keys, so extras is empty
    // here (see the module docs).
    let report =
        preset_io::export_preset(&project, &dest_dir, &BTreeMap::new()).map_err(to_typed_error)?;
    Ok(ExportResult {
        preset_path: report.preset_path.to_string_lossy().into_owned(),
        pass_files: report.pass_files,
        texture_files: report.texture_files,
        warnings: report.warnings,
    })
}

/// Validate a project for export (#64), returning the structured blockers the
/// export dialog lists. The frontend calls this to disable "Export" while the
/// project is not exportable and show the exact reasons (links into the Problems
/// panel). Never fails — an exportable project returns an empty blocker list
/// ([`ExportValidation::ok`]).
#[tauri::command]
pub fn validate_export(project: core_model::Project) -> ExportValidation {
    preset_io::validate_for_export(&project)
}

/// Map a non-`ok` [`ExportValidation`] onto the matching typed [`ExportError`] for
/// the fail-closed gate inside [`export_preset`]. Returns `None` when the project
/// is exportable. The first blocker decides the error: a graph pass becomes
/// `GraphPassUnsupported` (the writer's own error, raised before any write), and
/// the other structural blockers (no passes / empty source) become an `Io`-shaped
/// message — they are pre-write refusals, not OS errors, but reuse the existing
/// typed surface rather than widening the IPC error enum.
fn blocker_to_error(validation: &ExportValidation) -> Option<ExportError> {
    use core_model::ExportBlocker;
    validation.blockers.first().map(|b| match b {
        ExportBlocker::UncompiledGraphPass { pass_id, .. } => ExportError::GraphPassUnsupported {
            pass_id: pass_id.clone(),
        },
        other => ExportError::Io {
            message: format!("refusing to export: {other}"),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercise the command body headlessly (no Tauri runtime): import a fixture
    /// preset into a project, then export it and assert the bundle structure plus
    /// the no-absolute-paths contract — the acceptance test for #36's command.
    #[test]
    fn export_command_writes_bundle_with_relative_paths() {
        // Build a small source preset on disk and import it to a Project.
        let src = tempfile::tempdir().unwrap();
        std::fs::write(
            src.path().join("a.slang"),
            "#version 450\n#pragma parameter B \"B\" 1.0 0.0 2.0 0.01\nvoid main(){}\n",
        )
        .unwrap();
        std::fs::write(src.path().join("border.png"), b"\x89PNG\r\n").unwrap();
        std::fs::write(
            src.path().join("p.slangp"),
            "shaders = 1\n\
             shader0 = a.slang\n\
             scale_type0 = source\n\
             scale0 = 1.0\n\
             textures = BORDER\n\
             BORDER = border.png\n\
             B = 1.5\n",
        )
        .unwrap();
        let (project, _) = preset_io::import_preset(src.path().join("p.slangp")).expect("import");

        // Run the command body.
        let out = tempfile::tempdir().unwrap();
        let result =
            export_preset(project, out.path().to_string_lossy().into_owned()).expect("export");

        // Directory structure.
        assert!(out.path().join("preset.slangp").is_file());
        assert_eq!(result.pass_files, vec!["a.slang"]);
        assert!(out.path().join("a.slang").is_file());
        assert_eq!(result.texture_files.len(), 1);
        assert!(out
            .path()
            .join("textures")
            .join(&result.texture_files[0])
            .is_file());

        // No absolute paths in the emitted preset.
        let text = std::fs::read_to_string(out.path().join("preset.slangp")).unwrap();
        for line in text.lines() {
            let value = line.split_once('=').map(|(_, v)| v.trim()).unwrap_or("");
            assert!(
                !value.starts_with('/'),
                "value is an absolute path: {line:?}"
            );
        }
        assert!(!text.contains(&*src.path().to_string_lossy()));
        assert!(text.contains("shader0 = a.slang"));
        assert!(text.contains("BORDER = textures/border.png"));
        assert!(text.contains("B = 1.5"), "param override emitted inline");
    }

    /// A graph pass cannot be exported until Phase-5 codegen lands; the command
    /// must surface the typed [`ExportError::GraphPassUnsupported`] variant — not
    /// an opaque string — carrying the offending pass id, so the webview can branch
    /// on the expected limitation (Fix C1 acceptance).
    #[test]
    fn export_command_returns_typed_graph_pass_unsupported() {
        use core_model::{Graph, Pass, PassSettings, PassSource, Project};

        let project = Project {
            passes: vec![Pass {
                id: "graph-pass".to_owned(),
                name: "Graph".to_owned(),
                source: PassSource::Graph {
                    graph: Graph::default(),
                },
                parameters: vec![],
                references: vec![],
                settings: PassSettings::default(),
            }],
            ..Project::empty("Graph Export")
        };

        let out = tempfile::tempdir().unwrap();
        let err = export_preset(project, out.path().to_string_lossy().into_owned())
            .expect_err("graph pass must not export");

        // Branchable typed variant, carrying the pass id — not a flattened string.
        match err {
            ExportError::GraphPassUnsupported { pass_id } => assert_eq!(pass_id, "graph-pass"),
            other => panic!("expected GraphPassUnsupported, got {other:?}"),
        }
    }

    /// The fail-closed gate (#64): a project with a graph pass is refused BEFORE
    /// the writer runs, so NO bundle files (not even `preset.slangp`) are written.
    #[test]
    fn export_gate_writes_nothing_for_an_invalid_project() {
        use core_model::{Graph, Pass, PassSettings, PassSource, Project};

        let project = Project {
            passes: vec![Pass {
                id: "graph-pass".to_owned(),
                name: "Graph".to_owned(),
                source: PassSource::Graph {
                    graph: Graph::default(),
                },
                parameters: vec![],
                references: vec![],
                settings: PassSettings::default(),
            }],
            ..Project::empty("Invalid Export")
        };

        let out = tempfile::tempdir().unwrap();
        let err = export_preset(project, out.path().to_string_lossy().into_owned())
            .expect_err("invalid project must be refused");
        assert!(matches!(err, ExportError::GraphPassUnsupported { .. }));

        // The gate refused before invoking the writer: the destination is empty.
        let entries: Vec<_> = std::fs::read_dir(out.path()).unwrap().collect();
        assert!(
            entries.is_empty(),
            "no files must be written when the export gate refuses, found {} entries",
            entries.len()
        );
    }

    /// `validate_export` surfaces the structured blockers the dialog lists (#64).
    #[test]
    fn validate_export_reports_blockers() {
        use core_model::{ExportBlocker, Project};

        // An empty project has no passes to export.
        let v = validate_export(Project::empty("empty"));
        assert!(!v.ok());
        assert!(matches!(v.blockers.as_slice(), [ExportBlocker::NoPasses]));
    }
}
