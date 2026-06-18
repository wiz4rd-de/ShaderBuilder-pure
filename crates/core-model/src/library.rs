//! The on-disk **personal library store** (Phase 6, #58).
//!
//! The library persists reusable [`LibraryItem`]s (single nodes or whole
//! subgraphs, #56) so they survive restarts and can be reused across projects.
//! These are **pure** functions over a library *directory* (`&Path`) — exactly
//! the [`Project::save_to_file`](crate::Project::save_to_file) /
//! [`load_from_file`](crate::Project::load_from_file) pattern — so they are
//! unit-testable with a `tempfile::tempdir()` and **no** Tauri runtime. The thin
//! `#[tauri::command]` wrappers that resolve the per-user library dir live in the
//! `app` crate (`crates/app/src/library.rs`).
//!
//! ## Layout
//!
//! Each item is one JSON file `<dir>/<item.id>.json`. The id is the stable,
//! app-generated UUID-like string the store assigned ([`LibraryItem::id`]); those
//! ids are filename-safe (hex/`-`), so they map straight onto a filename with no
//! escaping. One-file-per-item means a corrupt file can be skipped without
//! losing the rest, and saving/deleting one item never rewrites the others.
//!
//! ## Atomicity
//!
//! [`save_item`] writes `<id>.json.tmp` then `std::fs::rename`s it over the final
//! path, so a reader (or a crash mid-write) never observes a half-written file —
//! the rename is atomic on the same filesystem.

use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::LibraryItem;

/// A typed error from the on-disk library store ([`save_item`] / [`list_items`] /
/// [`delete_item`] and the matching `*_library_node` Tauri commands, #58).
///
/// Mirrors [`ProjectLoadError`](crate::ProjectLoadError): `std::io::Error` is not
/// `Clone`/`Eq`/`Serialize`, so the [`Io`](LibraryError::Io) variant flattens a
/// failure to its `ErrorKind` label + message, keeping the whole enum a clean,
/// serializable IPC payload the frontend can match on. Note [`list_items`] never
/// returns this for a single corrupt file — it skips and logs that and carries on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum LibraryError {
    /// A filesystem operation failed (create the dir, write, rename, read the
    /// directory, remove a file, …).
    Io {
        /// The `std::io::ErrorKind` label (e.g. `"PermissionDenied"`). Named
        /// `errorKind` to avoid colliding with the `"kind"` serde tag.
        error_kind: String,
        /// The OS error message.
        message: String,
    },
    /// An item could not be **serialized** to JSON for storage (should not happen
    /// for a well-formed [`LibraryItem`]; present so [`save_item`] never panics).
    Malformed {
        /// The underlying `serde_json` message.
        message: String,
    },
    /// [`delete_item`] was asked to remove an item id that has no file on disk.
    NotFound {
        /// The id that was not found.
        id: String,
    },
}

impl std::fmt::Display for LibraryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LibraryError::Io {
                error_kind,
                message,
            } => write!(f, "library IO error ({error_kind}): {message}"),
            LibraryError::Malformed { message } => {
                write!(f, "could not serialize library item: {message}")
            }
            LibraryError::NotFound { id } => {
                write!(f, "library item not found: {id}")
            }
        }
    }
}

impl std::error::Error for LibraryError {}

/// Build an [`LibraryError::Io`] from a `std::io::Error`.
fn io_err(e: std::io::Error) -> LibraryError {
    LibraryError::Io {
        error_kind: format!("{:?}", e.kind()),
        message: e.to_string(),
    }
}

/// The on-disk path of `id` inside `dir`: `<dir>/<id>.json`.
fn item_path(dir: &Path, id: &str) -> std::path::PathBuf {
    dir.join(format!("{id}.json"))
}

/// Save `item` to `<dir>/<item.id>.json`, **atomically** (#58).
///
/// Creates `dir` (and any missing parents) if absent, serializes `item` to pretty
/// JSON, writes it to a `<id>.json.tmp` sibling, then `rename`s that over the final
/// path so a concurrent reader or a crash mid-write never sees a partial file.
/// The filename is derived from the stable, filename-safe [`LibraryItem::id`].
/// Returns a typed [`LibraryError`] (a serialize failure or an IO failure) instead
/// of panicking.
pub fn save_item(dir: &Path, item: &LibraryItem) -> Result<(), LibraryError> {
    std::fs::create_dir_all(dir).map_err(io_err)?;

    let mut json =
        serde_json::to_string_pretty(item).map_err(|e| LibraryError::Malformed {
            message: e.to_string(),
        })?;
    json.push('\n');

    let final_path = item_path(dir, &item.id);
    let tmp_path = dir.join(format!("{}.json.tmp", item.id));
    std::fs::write(&tmp_path, json).map_err(io_err)?;
    std::fs::rename(&tmp_path, &final_path).map_err(io_err)
}

/// List every library item stored in `dir`, in a deterministic order (#58).
///
/// If `dir` does not exist, returns an empty `Vec` (an empty library is not an
/// error). Otherwise enumerates `*.json` files and deserializes each into a
/// [`LibraryItem`]. A file that fails to read or parse (corrupt, hand-edited, or
/// an unrelated `.json`) is **skipped** with a logged warning — it never aborts
/// the whole call. The result is sorted by `(name, id)` so the UI order is stable.
pub fn list_items(dir: &Path) -> Result<Vec<LibraryItem>, LibraryError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        // A missing library dir simply means "no items yet".
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_err(e)),
    };

    let mut items = Vec::new();
    for entry in entries {
        let entry = entry.map_err(io_err)?;
        let path = entry.path();
        // Only consider `*.json` files (skips the `*.json.tmp` of an in-flight
        // save and any non-JSON files).
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }

        let json = match std::fs::read_to_string(&path) {
            Ok(json) => json,
            Err(e) => {
                eprintln!(
                    "library: skipping unreadable item file {}: {e}",
                    path.display()
                );
                continue;
            }
        };
        match serde_json::from_str::<LibraryItem>(&json) {
            Ok(item) => items.push(item),
            Err(e) => {
                eprintln!(
                    "library: skipping malformed item file {}: {e}",
                    path.display()
                );
            }
        }
    }

    items.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    Ok(items)
}

/// Delete the library item with id `id` from `dir` (#58).
///
/// Removes `<dir>/<id>.json`. Returns [`LibraryError::NotFound`] if no such file
/// exists (so the caller can tell "deleted" from "wasn't there"), and any other
/// removal failure as [`LibraryError::Io`].
pub fn delete_item(dir: &Path, id: &str) -> Result<(), LibraryError> {
    let path = item_path(dir, id);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(LibraryError::NotFound { id: id.to_owned() })
        }
        Err(e) => Err(io_err(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LibraryPayload, Node, Vec2};

    /// A minimal single-node library item with a filename-safe, UUID-like id.
    fn sample_item(id: &str, name: &str) -> LibraryItem {
        LibraryItem {
            id: id.to_owned(),
            name: name.to_owned(),
            description: None,
            tags: vec![],
            payload: LibraryPayload::Node {
                node: Node {
                    id: "n0".to_owned(),
                    kind: "const".to_owned(),
                    position: Vec2 { x: 0.0, y: 0.0 },
                    data: serde_json::json!({}),
                },
            },
        }
    }

    #[test]
    fn save_then_list_returns_the_item() {
        let dir = tempfile::tempdir().expect("tempdir");
        let item = sample_item("11111111-1111-4111-8111-111111111111", "Const Zero");

        save_item(dir.path(), &item).expect("save_item");
        let listed = list_items(dir.path()).expect("list_items");

        assert_eq!(listed, vec![item]);
    }

    #[test]
    fn save_writes_a_single_json_file_named_for_the_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let item = sample_item("abc-123", "Named");
        save_item(dir.path(), &item).expect("save_item");

        assert!(dir.path().join("abc-123.json").is_file());
        // No leftover temp file from the atomic write.
        assert!(!dir.path().join("abc-123.json.tmp").exists());
    }

    #[test]
    fn list_on_missing_dir_returns_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("does-not-exist");
        let listed = list_items(&missing).expect("list_items on missing dir");
        assert!(listed.is_empty());
    }

    #[test]
    fn save_then_delete_then_list_omits_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let keep = sample_item("keep-1", "Keep");
        let drop = sample_item("drop-1", "Drop");
        save_item(dir.path(), &keep).expect("save keep");
        save_item(dir.path(), &drop).expect("save drop");

        delete_item(dir.path(), "drop-1").expect("delete_item");

        let listed = list_items(dir.path()).expect("list_items");
        assert_eq!(listed, vec![keep]);
    }

    #[test]
    fn delete_missing_id_returns_not_found() {
        let dir = tempfile::tempdir().expect("tempdir");
        match delete_item(dir.path(), "nope") {
            Err(LibraryError::NotFound { id }) => assert_eq!(id, "nope"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn list_skips_corrupt_file_but_returns_good_items() {
        let dir = tempfile::tempdir().expect("tempdir");
        let good = sample_item("good-1", "Good");
        save_item(dir.path(), &good).expect("save good");

        // A hand-written corrupt JSON file placed in the dir.
        std::fs::write(dir.path().join("corrupt-1.json"), b"{ not json")
            .expect("write corrupt fixture");

        let listed = list_items(dir.path()).expect("list_items");
        assert_eq!(listed, vec![good]);
    }

    #[test]
    fn list_is_sorted_deterministically_by_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Save out of order; expect sorted by name.
        save_item(dir.path(), &sample_item("z-id", "Zebra")).expect("save z");
        save_item(dir.path(), &sample_item("a-id", "Apple")).expect("save a");
        save_item(dir.path(), &sample_item("m-id", "Mango")).expect("save m");

        let names: Vec<String> = list_items(dir.path())
            .expect("list_items")
            .into_iter()
            .map(|i| i.name)
            .collect();
        assert_eq!(names, vec!["Apple", "Mango", "Zebra"]);
    }

    #[test]
    fn save_then_list_survives_a_simulated_restart() {
        // "Restart" = a fresh list_items(same_dir) with NO shared in-memory state.
        // The same directory still yields the saved items.
        let dir = tempfile::tempdir().expect("tempdir");
        let item = sample_item("persist-1", "Persisted");
        save_item(dir.path(), &item).expect("save_item");

        // Simulate the process going away: nothing is held in memory; we just call
        // the pure list fn again against the same on-disk dir.
        let after_restart = list_items(dir.path()).expect("list_items after restart");
        assert_eq!(after_restart, vec![item]);
    }
}
