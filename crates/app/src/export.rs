//! The `export_preset` Tauri command (#36): write a frontend-owned
//! [`core_model::Project`] out as a RetroArch-conventional `.slangp` bundle.
//!
//! The heavy lifting — directory layout, byte-exact pass `.slang` files, LUT
//! copying, relative paths, inline parameter defaults, preserved-key re-emission —
//! lives in [`preset_io::export_preset`]. This module is the thin IPC seam: it
//! takes the project the editor holds plus a destination directory, calls the
//! writer, and returns a small JSON-friendly summary the UI can show.
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

use serde::Serialize;

/// The summary [`export_preset`] returns to the webview: where the bundle was
/// written and what files it contains, plus any non-fatal notes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    /// Absolute path of the written `preset.slangp`.
    pub preset_path: String,
    /// Per-pass `.slang` file names written, relative to the bundle root.
    pub pass_files: Vec<String>,
    /// LUT file names written under `textures/`, relative to the bundle root.
    pub texture_files: Vec<String>,
    /// Non-fatal notes (e.g. a LUT source image that could not be copied in).
    pub warnings: Vec<String>,
}

/// Export the editor's current [`core_model::Project`] as a RetroArch bundle under
/// `dest_dir` (#36). Writes `preset.slangp` + per-pass `.slang` + `textures/` LUT
/// PNGs with **relative** paths and inline parameter defaults; returns a summary.
/// A write error (bad destination, unwritable dir, a graph pass) is returned to
/// the caller as a string.
#[tauri::command]
pub fn export_preset(
    project: core_model::Project,
    dest_dir: String,
) -> Result<ExportResult, String> {
    // The editable project carries no preserved unknown keys, so extras is empty
    // here (see the module docs).
    let report = preset_io::export_preset(&project, &dest_dir, &BTreeMap::new())
        .map_err(|e| e.to_string())?;
    Ok(ExportResult {
        preset_path: report.preset_path.to_string_lossy().into_owned(),
        pass_files: report.pass_files,
        texture_files: report.texture_files,
        warnings: report.warnings,
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
}
