//! Native **personal library** persistence commands (Phase 6, #58).
//!
//! Thin `#[tauri::command]` wrappers over the pure
//! [`core_model::library`] store fns ([`save_item`](core_model::save_item) /
//! [`list_items`](core_model::list_items) / [`delete_item`](core_model::delete_item)),
//! exactly mirroring the `project.rs` save/load pattern: the real logic is pure
//! and unit-tested in `core-model` against a `tempdir`; these wrappers only
//! resolve the per-user library directory and delegate.
//!
//! ## Where the library lives
//!
//! The dir is resolved **per call** from the [`AppHandle`] via Tauri v2's path
//! API — `app.path().app_data_dir()?` joined with `"library"` — not held in
//! managed state. Resolving fresh each call keeps the commands stateless: a
//! "restart" is just another `list_library_node` against the same on-disk dir
//! (the core-model test proves this).
//!
//! ## No fs-plugin permission needed
//!
//! All file IO happens in the Rust backend through `std::fs` (inside the
//! core-model store fns), so the commands need no Tauri fs-plugin capability —
//! the app's capabilities stay at `core:default`.

use core_model::{LibraryError, LibraryItem};
use tauri::{AppHandle, Manager};

/// Resolve `<app_data_dir>/library` for this user, surfacing a path-resolution
/// failure as a [`LibraryError::Io`] so the command never panics.
fn library_dir(app: &AppHandle) -> Result<std::path::PathBuf, LibraryError> {
    app.path()
        .app_data_dir()
        .map(|dir| dir.join("library"))
        .map_err(|e| LibraryError::Io {
            error_kind: "NotFound".to_owned(),
            message: format!("could not resolve app data dir: {e}"),
        })
}

/// Persist a library item to the per-user library dir (#58).
///
/// Resolves the dir from `app`, then delegates to [`core_model::save_item`],
/// which atomically writes `<id>.json`. Returns a typed [`LibraryError`].
#[tauri::command]
pub fn save_library_node(app: AppHandle, item: LibraryItem) -> Result<(), LibraryError> {
    let dir = library_dir(&app)?;
    core_model::save_item(&dir, &item)
}

/// List every persisted library item for this user (#58).
///
/// Resolves the dir and delegates to [`core_model::list_items`] (empty when the
/// library is empty/missing; corrupt files are skipped, never fatal).
#[tauri::command]
pub fn list_library_node(app: AppHandle) -> Result<Vec<LibraryItem>, LibraryError> {
    let dir = library_dir(&app)?;
    core_model::list_items(&dir)
}

/// Delete a persisted library item by id (#58).
///
/// Resolves the dir and delegates to [`core_model::delete_item`], which returns
/// [`LibraryError::NotFound`] if no such item exists.
#[tauri::command]
pub fn delete_library_node(app: AppHandle, id: String) -> Result<(), LibraryError> {
    let dir = library_dir(&app)?;
    core_model::delete_item(&dir, &id)
}
