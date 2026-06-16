//! ShaderBuilder Tauri application.
//!
//! Phase 0: builds the Tauri context and opens a single window hosting the
//! React/React Flow frontend (Architecture §A). The Rust↔web command surface
//! and the `tauri::ipc::Channel` binary frame path land in #13.

/// The workspace engine crates wired into the app. Phase 0 keeps the
/// `app` → all dependency edges (Architecture §B) live and referenced until
/// later phases give each crate a real API to call.
fn linked_crates() -> [&'static str; 8] {
    [
        core_model::NAME,
        ir::NAME,
        codegen_slang::NAME,
        codegen_glslp::NAME,
        slang_compile::NAME,
        preview_engine::NAME,
        source::NAME,
        preset_io::NAME,
    ]
}

/// Build the Tauri context and run the application.
pub fn run() {
    // Surface the wired-in engine crates in dev logs; replaced by real
    // command/render wiring in later phases.
    if cfg!(debug_assertions) {
        eprintln!(
            "ShaderBuilder linked crates: {}",
            linked_crates().join(", ")
        );
    }

    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running ShaderBuilder");
}

#[cfg(test)]
mod tests {
    use super::linked_crates;

    #[test]
    fn all_engine_crates_are_linked() {
        let crates = linked_crates();
        assert_eq!(crates.len(), 8);
        assert!(crates.contains(&"core-model"));
        assert!(crates.contains(&"preview-engine"));
        assert!(crates.contains(&"preset-io"));
    }
}
