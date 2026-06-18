//! ShaderBuilder Tauri application.
//!
//! Phase 0: builds the Tauri context, opens a single window hosting the
//! React/React Flow frontend (Architecture §A), and stands up the **binary
//! frame transport** — a `tauri::ipc::Channel` carrying raw RGBA frames from
//! Rust to a `<canvas>` (Architecture §E/§F). The frame *producer* is a dummy
//! gradient ([`preview_engine::GradientSource`]); Phase 1 swaps in the offscreen
//! wgpu renderer behind the same [`preview_engine::FrameSource`] seam **without
//! touching this transport**.

mod export;
mod graph;
mod library;
mod preview;
mod project;
mod scan;
mod session;

use tauri::{Emitter, Manager};

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
        .plugin(tauri_plugin_dialog::init())
        .manage(preview::PreviewState::default())
        .manage(session::DirtyState::default())
        // Window close with unsaved edits (#63): the close handler runs in the Rust
        // event loop and cannot read the JS dirty flag, so the frontend mirrors it
        // into the managed `DirtyState` mutex via `set_dirty`. When dirty, we veto
        // the close (`prevent_close`) and ask the frontend to run the
        // save/discard/cancel prompt; the frontend re-issues the close once the user
        // has chosen (clearing dirty first, so this handler then lets it through).
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let dirty = window
                    .state::<session::DirtyState>()
                    .0
                    .lock()
                    .map(|g| *g)
                    .unwrap_or(false);
                if dirty {
                    api.prevent_close();
                    let _ = window.emit("close-requested", ());
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            preview::start_preview_stream,
            preview::stop_preview_stream,
            preview::load_source,
            preview::load_test_pattern,
            preview::load_source_sequence,
            preview::load_shader,
            preview::load_shader_source,
            preview::load_chain_sources,
            preview::load_preset,
            preview::set_viewport,
            preview::set_simulated_viewport,
            preview::set_parameter,
            preview::inspect_pixel,
            preview::play,
            preview::pause,
            preview::step,
            preview::seek,
            preview::set_fps,
            export::export_preset,
            project::save_project,
            project::load_project,
            graph::compile_graph,
            scan::scan_pass_source,
            library::save_library_node,
            library::list_library_node,
            library::delete_library_node,
            session::set_dirty,
            session::load_recents,
            session::push_recent,
            session::autosave_recovery,
            session::clear_recovery,
            session::check_recovery,
        ])
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
