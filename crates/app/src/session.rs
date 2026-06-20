//! Session commands: **recent-projects list**, **autosave recovery**, and the
//! **backend-visible dirty flag** (Phase 7, #63).
//!
//! Thin `#[tauri::command]` wrappers over the pure [`core_model::session`] store
//! fns, mirroring `library.rs`: the real logic is pure + unit-tested in
//! `core-model` against a `tempdir`; these wrappers only resolve the per-user
//! config directory (`app_data_dir()`) and delegate.
//!
//! ## The backend-visible dirty flag
//!
//! The window close handler ([`crate::run`]'s `on_window_event`) runs in the Rust
//! event loop and **cannot read the JS-side dirty boolean** — so the frontend
//! mirrors its dirty state to a managed `Mutex<bool>` ([`DirtyState`]) via the
//! tiny [`set_dirty`] command on every change. The close handler reads that mutex
//! to decide whether to prompt for unsaved changes.
//!
//! ## No fs-plugin permission needed
//!
//! All file IO happens in the Rust backend through `std::fs` (inside the
//! core-model store fns), so these commands need no Tauri fs-plugin capability.

use core_model::{Project, RecentProject, Recovery, SessionError};
use tauri::{AppHandle, Manager};

/// The backend mirror of the frontend's unsaved-changes flag (#63), updated by
/// [`set_dirty`] and read by the window-close handler (which cannot see the JS
/// flag). Managed as Tauri state.
#[derive(Default)]
pub struct DirtyState(pub std::sync::Mutex<bool>);

/// Resolve `<app_data_dir>` for this user, surfacing a path-resolution failure as
/// a [`SessionError::Io`] so the command never panics.
fn config_dir(app: &AppHandle) -> Result<std::path::PathBuf, SessionError> {
    app.path().app_data_dir().map_err(|e| SessionError::Io {
        error_kind: "NotFound".to_owned(),
        message: format!("could not resolve app data dir: {e}"),
    })
}

/// Mirror the frontend's unsaved-changes flag into the backend (#63).
///
/// Called by the document store whenever `dirty` changes so the window-close
/// handler can read a Rust-visible value (it cannot reach into the JS state).
#[tauri::command]
pub fn set_dirty(dirty: bool, state: tauri::State<'_, DirtyState>) {
    if let Ok(mut guard) = state.0.lock() {
        *guard = dirty;
    }
}

/// Load the recent-projects list (#63), pruning entries whose file no longer
/// exists. Most-recent first.
#[tauri::command]
pub fn load_recents(app: AppHandle) -> Result<Vec<RecentProject>, SessionError> {
    let dir = config_dir(&app)?;
    Ok(core_model::load_recents(&dir))
}

/// Push a freshly-opened/saved project to the front of the recents list (#63),
/// returning the updated list.
#[tauri::command]
pub fn push_recent(
    app: AppHandle,
    entry: RecentProject,
) -> Result<Vec<RecentProject>, SessionError> {
    let dir = config_dir(&app)?;
    core_model::push_recent(&dir, entry)
}

/// Autosave the live working document to the recovery file (#63).
///
/// `projectPath` is the file the document is associated with (or `null` for a new
/// untitled document). Stamps the autosave with the current wall-clock time.
#[tauri::command]
pub fn autosave_recovery(
    app: AppHandle,
    project: Project,
    project_path: Option<String>,
) -> Result<(), SessionError> {
    let dir = config_dir(&app)?;
    core_model::write_recovery(&dir, &project, project_path, core_model::session::now_ms())
}

/// Clear the recovery files (#63) — called after a successful save, or once the
/// user accepts/declines a restore.
#[tauri::command]
pub fn clear_recovery(app: AppHandle) -> Result<(), SessionError> {
    let dir = config_dir(&app)?;
    core_model::clear_recovery(&dir)
}

/// On launch, return a recovery offer (#63) ONLY when a recovery file exists AND
/// it is strictly newer than the last saved version of its project (a never-saved
/// document's recovery is always offered). Returns `None` when there is nothing
/// worth restoring, so the frontend can show a restore prompt iff `Some`.
#[tauri::command]
pub fn check_recovery(app: AppHandle) -> Result<Option<Recovery>, SessionError> {
    let dir = config_dir(&app)?;
    let Some(recovery) = core_model::read_recovery(&dir)? else {
        return Ok(None);
    };
    let last_saved = recovery
        .meta
        .project_path
        .as_deref()
        .and_then(core_model::file_modified_ms);
    if core_model::recovery_is_newer(recovery.meta.saved_at_ms, last_saved) {
        Ok(Some(recovery))
    } else {
        Ok(None)
    }
}
