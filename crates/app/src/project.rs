//! Native **project file** persistence commands (Spec §6, #38).
//!
//! [`save_project`] and [`load_project`] are the thin Tauri command wrappers
//! around [`core_model::Project::save_to_file`] / [`core_model::Project::load_from_file`]:
//! they read/write **one** self-contained `.json` document (node graphs +
//! pipeline + parameters + metadata + library refs) and return the **typed**
//! [`core_model::ProjectSaveError`] / [`core_model::ProjectLoadError`] on failure
//! — never a panic, even for a missing, malformed, or out-of-date file
//! (Architecture §E, Spec §6 acceptance).
//!
//! ## The export boundary (enforced here)
//!
//! The native project file is **strictly separate** from the exported RetroArch
//! `.slangp` bundle (Spec §6). These commands touch *only* the JSON document:
//! they never read or write a `.slangp`, copy LUT PNGs, or emit per-pass `.slang`
//! files. Exporting a runnable bundle is a wholly different path —
//! `preset_io::export_preset` (#36) — reached by a different command. Because the
//! two live in different functions in different crates with non-overlapping IO,
//! the boundary can't be crossed by accident. The Phase-7 save/load UX (#63) wraps
//! these two commands directly and needs no new persistence logic.

use core_model::{Project, ProjectLoadError, ProjectSaveError};

/// Save a project to a single native `.json` file (Spec §6, #38).
///
/// Serializes `project` to the versioned project-file JSON (carrying
/// `schemaVersion` for later migration) and writes it to `path`. Returns a typed
/// [`ProjectSaveError`] (an IO write failure, or — effectively never — a
/// serialize failure) rather than panicking. Writes **only** the JSON document;
/// it does not produce or touch any `.slangp` export bundle.
#[tauri::command]
pub fn save_project(path: String, project: Project) -> Result<(), ProjectSaveError> {
    project.save_to_file(&path)
}

/// Load a project from a single native `.json` file (Spec §6, #38).
///
/// Reads `path`, parses it, and **version-validates** it: a read failure, a
/// malformed document, or an out-of-date / too-new `schemaVersion` each surfaces
/// as the corresponding typed [`ProjectLoadError`] variant instead of a panic
/// (Spec §6 acceptance). On success returns the in-memory [`Project`] — the same
/// serde type used across IPC, so no conversion happens at the boundary.
#[tauri::command]
pub fn load_project(path: String) -> Result<Project, ProjectLoadError> {
    Project::load_from_file(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small multi-pass project covering the shapes #38 must round-trip: a
    /// whole-pass code node, a graph pass, project + per-pass parameters, and
    /// project metadata.
    fn sample_project() -> Project {
        use core_model::{
            Graph, Parameter, Pass, PassSettings, PassSource, ProjectMetadata,
            PROJECT_SCHEMA_VERSION,
        };
        Project {
            schema_version: PROJECT_SCHEMA_VERSION,
            name: "Command Round Trip".to_owned(),
            metadata: ProjectMetadata {
                description: Some("via save_project/load_project".to_owned()),
                ..ProjectMetadata::default()
            },
            parameters: vec![Parameter {
                name: "GAIN".to_owned(),
                label: "Gain".to_owned(),
                default: 1.0,
                min: 0.0,
                max: 4.0,
                step: 0.1,
            }],
            passes: vec![
                Pass {
                    id: "p0".to_owned(),
                    name: "First".to_owned(),
                    source: PassSource::WholePassCode {
                        source: "// pass 0\r\nvoid main() {}\n".to_owned(),
                        filename: Some("p0.slang".to_owned()),
                        opaque: true,
                    },
                    parameters: vec![],
                    references: vec![],
                    settings: PassSettings::default(),
                },
                Pass {
                    id: "p1".to_owned(),
                    name: "Second".to_owned(),
                    source: PassSource::Graph {
                        graph: Graph::default(),
                    },
                    parameters: vec![Parameter {
                        name: "GAIN".to_owned(),
                        label: "Gain".to_owned(),
                        default: 1.0,
                        min: 0.0,
                        max: 4.0,
                        step: 0.1,
                    }],
                    references: vec![],
                    settings: PassSettings::default(),
                },
            ],
            ..Project::empty("Command Round Trip")
        }
    }

    fn temp_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "sb-cmd-{tag}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn save_then_load_round_trips_via_commands() {
        // Exercise the exact command bodies (no Tauri runtime needed — they take
        // plain args), proving save_project -> load_project yields an identical
        // in-memory model (#38 acceptance).
        let project = sample_project();
        let path = temp_path("rt");

        save_project(path.to_string_lossy().into_owned(), project.clone()).expect("save_project");
        let loaded = load_project(path.to_string_lossy().into_owned()).expect("load_project");
        assert_eq!(project, loaded);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_project_missing_file_returns_typed_io_error() {
        let path = temp_path("missing");
        let _ = std::fs::remove_file(&path);
        match load_project(path.to_string_lossy().into_owned()) {
            Err(ProjectLoadError::Io { error_kind, .. }) => assert_eq!(error_kind, "NotFound"),
            other => panic!("expected Io(NotFound), got {other:?}"),
        }
    }

    #[test]
    fn load_project_malformed_file_returns_typed_error_not_panic() {
        let path = temp_path("malformed");
        std::fs::write(&path, b"{ this is not json").expect("write fixture");
        let result = load_project(path.to_string_lossy().into_owned());
        assert!(
            matches!(result, Err(ProjectLoadError::Malformed { .. })),
            "expected Malformed, got {result:?}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_project_out_of_date_version_returns_typed_error() {
        // A v0 file is older than the current schema with no migration registered.
        let path = temp_path("v0");
        std::fs::write(&path, br#"{"schemaVersion":0,"name":"x","passes":[]}"#)
            .expect("write fixture");
        match load_project(path.to_string_lossy().into_owned()) {
            Err(ProjectLoadError::Unsupported { found, .. }) => assert_eq!(found, 0),
            other => panic!("expected Unsupported, got {other:?}"),
        }
        let _ = std::fs::remove_file(&path);
    }
}
