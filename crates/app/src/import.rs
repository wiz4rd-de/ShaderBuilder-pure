//! The `import_preset` Tauri command (#66): read a RetroArch `.slangp` preset
//! from disk and lower it into the editor's [`core_model::Project`], so the
//! onboarding start screen's "Import preset…" action (and a future File ▸ Import)
//! can bring an external preset into the editor as whole-pass code passes.
//!
//! The lowering — parsing the `.slangp`, reading each `.slang` byte-for-byte,
//! recovering `#pragma parameter` knobs, classifying texture references, mapping
//! LUTs — already lives in [`preset_io::import_preset`]; this module is the thin
//! IPC seam. It returns the [`core_model::Project`] (the same serde type used
//! everywhere else) on success, or the parse error flattened to a string on
//! failure (a malformed/missing preset). Import DIAGNOSTICS (preserved unknown
//! keys, unreadable shaders) are non-fatal and not surfaced here — the imported
//! project is usable; the frontend toasts a generic note rather than a structured
//! list, keeping this command's surface minimal.

/// Import a RetroArch `.slangp` preset at `path` into a [`core_model::Project`]
/// (#66). Returns the project on success, or a human-readable message on a parse
/// failure (missing `shaders` key, bad scale type, unreadable preset …).
#[tauri::command]
pub fn import_preset(path: String) -> Result<core_model::Project, String> {
    let (project, _diagnostics) =
        preset_io::import_preset(&path).map_err(|e| format!("could not import preset: {e}"))?;
    Ok(project)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::PassSource;

    #[test]
    fn imports_a_two_pass_preset_to_whole_pass_code() {
        // A minimal on-disk preset (no GPU, no Tauri runtime — the command takes a
        // plain path arg) exercising the command body end to end.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.slang"),
            "#version 450\n#pragma parameter K \"K\" 0.5 0.0 1.0 0.1\nvoid main(){}\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("b.slang"), "#version 450\nvoid main(){}\n").unwrap();
        std::fs::write(
            dir.path().join("p.slangp"),
            "shaders = 2\nshader0 = a.slang\nshader1 = b.slang\n",
        )
        .unwrap();

        let project = import_preset(dir.path().join("p.slangp").to_string_lossy().into_owned())
            .expect("import");
        assert_eq!(project.passes.len(), 2);
        assert!(matches!(
            project.passes[0].source,
            PassSource::WholePassCode { .. }
        ));
        // The pragma parameter reached the project level.
        assert!(project.parameters.iter().any(|p| p.name == "K"));
    }

    #[test]
    fn a_missing_preset_returns_a_message_not_a_panic() {
        let err = import_preset("/no/such/preset.slangp".to_owned()).unwrap_err();
        assert!(err.contains("could not import preset"), "got: {err}");
    }
}
