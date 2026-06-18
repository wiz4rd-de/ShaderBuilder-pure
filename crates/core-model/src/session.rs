//! Session persistence: the **recent-projects list** and **autosave recovery**
//! (Phase 7, #63).
//!
//! These are **pure** functions over a config *directory* (`&Path`) — exactly the
//! [`Project::save_to_file`](crate::Project::save_to_file) /
//! [`crate::library`] store pattern — so they unit-test with a
//! `tempfile::tempdir()` and **no** Tauri runtime. The thin `#[tauri::command]`
//! wrappers that resolve the per-user `app_data_dir()` live in the `app` crate
//! (`crates/app/src/session.rs`).
//!
//! ## Recents (`<dir>/recents.json`)
//!
//! A small JSON array of [`RecentProject`] entries, most-recently-opened first.
//! [`push_recent`] de-dupes by path (case-sensitively), moves an existing entry
//! to the front, and caps the list at [`MAX_RECENTS`]. [`load_recents`] drops any
//! entry whose file no longer exists (so a list of stale paths self-heals) — a
//! missing recents file is simply an empty list, never an error.
//!
//! ## Autosave recovery (`<dir>/recovery/working.json` + `…/meta.json`)
//!
//! [`write_recovery`] mirrors the live working document to a recovery file and
//! stamps a sibling `meta.json` with the path the document came from and a
//! millisecond timestamp. [`read_recovery`] reads it back; [`recovery_is_newer`]
//! compares the recovery timestamp against the timestamp the document was last
//! *saved* so the launcher can decide whether to offer a restore. [`clear_recovery`]
//! removes the recovery files after a successful save or once the user declines or
//! accepts the restore.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::Project;

/// How many entries the recent-projects list retains (older entries fall off).
pub const MAX_RECENTS: usize = 12;

/// The recents-list filename inside the config dir.
const RECENTS_FILE: &str = "recents.json";
/// The recovery subdirectory inside the config dir.
const RECOVERY_DIR: &str = "recovery";
/// The recovered working-document filename inside the recovery dir.
const RECOVERY_DOC: &str = "working.json";
/// The recovery metadata filename inside the recovery dir.
const RECOVERY_META: &str = "meta.json";

/// One entry in the recent-projects list (#63): the absolute project-file path
/// plus the display name captured at the time it was opened/saved, so the File
/// menu can show a friendly label without re-reading every file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecentProject {
    /// The absolute path to the `.json` project file.
    pub path: String,
    /// The project's display name at open/save time (best-effort label).
    pub name: String,
}

/// A typed error from the session store (#63), mirroring
/// [`LibraryError`](crate::LibraryError): `std::io::Error` is not
/// `Clone`/`Eq`/`Serialize`, so [`Io`](SessionError::Io) flattens a failure to its
/// `ErrorKind` label + message, keeping the enum a clean, serializable IPC payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum SessionError {
    /// A filesystem operation failed (create the dir, write, rename, read, …).
    Io {
        /// The `std::io::ErrorKind` label (e.g. `"PermissionDenied"`).
        error_kind: String,
        /// The OS error message.
        message: String,
    },
    /// A stored session file could not be (de)serialized.
    Malformed {
        /// The underlying `serde_json` message.
        message: String,
    },
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::Io {
                error_kind,
                message,
            } => write!(f, "session IO error ({error_kind}): {message}"),
            SessionError::Malformed { message } => {
                write!(f, "malformed session file: {message}")
            }
        }
    }
}

impl std::error::Error for SessionError {}

/// Build a [`SessionError::Io`] from a `std::io::Error`.
fn io_err(e: std::io::Error) -> SessionError {
    SessionError::Io {
        error_kind: format!("{:?}", e.kind()),
        message: e.to_string(),
    }
}

/// Build a [`SessionError::Malformed`] from a `serde_json::Error`.
fn json_err(e: serde_json::Error) -> SessionError {
    SessionError::Malformed {
        message: e.to_string(),
    }
}

// --- recents ----------------------------------------------------------------

/// The `<dir>/recents.json` path.
fn recents_path(dir: &Path) -> PathBuf {
    dir.join(RECENTS_FILE)
}

/// Read the raw recents list from `<dir>/recents.json` WITHOUT pruning missing
/// files — the internal building block for [`load_recents`] / [`push_recent`].
/// A missing file is an empty list; a malformed file is treated as empty (so a
/// hand-corrupted recents file never bricks the app) rather than an error.
fn read_recents_raw(dir: &Path) -> Vec<RecentProject> {
    let path = recents_path(dir);
    let json = match std::fs::read_to_string(&path) {
        Ok(json) => json,
        Err(_) => return Vec::new(),
    };
    serde_json::from_str(&json).unwrap_or_default()
}

/// Write `recents` to `<dir>/recents.json` (pretty, trailing newline), creating
/// `dir` if absent.
fn write_recents(dir: &Path, recents: &[RecentProject]) -> Result<(), SessionError> {
    std::fs::create_dir_all(dir).map_err(io_err)?;
    let mut json = serde_json::to_string_pretty(recents).map_err(json_err)?;
    json.push('\n');
    std::fs::write(recents_path(dir), json).map_err(io_err)
}

/// Load the recent-projects list (#63), **pruning entries whose file no longer
/// exists** so a stale path self-heals out of the list. Most-recent first.
///
/// If pruning changed the list it is written back (best-effort: a write failure is
/// ignored, the pruned list is still returned). A missing/corrupt recents file is
/// an empty list, never an error.
pub fn load_recents(dir: &Path) -> Vec<RecentProject> {
    let raw = read_recents_raw(dir);
    let pruned: Vec<RecentProject> = raw
        .iter()
        .filter(|r| Path::new(&r.path).is_file())
        .cloned()
        .collect();
    if pruned.len() != raw.len() {
        let _ = write_recents(dir, &pruned);
    }
    pruned
}

/// Push `entry` to the FRONT of the recent-projects list (#63), de-duped by path:
/// an existing entry with the same path is removed first (so re-opening moves it to
/// the front and refreshes its name), then the list is capped at [`MAX_RECENTS`].
/// Returns the new list. Does NOT prune missing files (that is [`load_recents`]'s
/// job) so a just-saved file is never dropped by a race.
pub fn push_recent(dir: &Path, entry: RecentProject) -> Result<Vec<RecentProject>, SessionError> {
    let mut recents = read_recents_raw(dir);
    recents.retain(|r| r.path != entry.path);
    recents.insert(0, entry);
    recents.truncate(MAX_RECENTS);
    write_recents(dir, &recents)?;
    Ok(recents)
}

// --- autosave / recovery ----------------------------------------------------

/// Metadata stamped alongside an autosave recovery document (#63).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecoveryMeta {
    /// The project-file path the recovered document came from, or `None` for a
    /// never-saved (new, untitled) document.
    #[serde(default)]
    pub project_path: Option<String>,
    /// The project's display name at autosave time (for the restore prompt).
    pub name: String,
    /// Milliseconds since the Unix epoch when the autosave was written.
    pub saved_at_ms: u64,
}

/// A recovered autosave (#63): the working [`Project`] plus its [`RecoveryMeta`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Recovery {
    /// The recovered working document.
    pub project: Project,
    /// When/where it was autosaved.
    pub meta: RecoveryMeta,
}

/// The `<dir>/recovery/` directory.
fn recovery_dir(dir: &Path) -> PathBuf {
    dir.join(RECOVERY_DIR)
}

/// Milliseconds since the Unix epoch (saturating to 0 before the epoch).
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Write the live working document to the recovery files (#63): `recovery/working.json`
/// (the [`Project`]) and `recovery/meta.json` (its [`RecoveryMeta`]).
///
/// `project_path` is the file the document is associated with (or `None` for a new
/// untitled document); `saved_at_ms` is the autosave timestamp (pass [`now_ms`]).
/// Both files are written atomically (`*.tmp` then `rename`) so a crash mid-write
/// never leaves a half-document the launcher would try to restore.
pub fn write_recovery(
    dir: &Path,
    project: &Project,
    project_path: Option<String>,
    saved_at_ms: u64,
) -> Result<(), SessionError> {
    let rdir = recovery_dir(dir);
    std::fs::create_dir_all(&rdir).map_err(io_err)?;

    let meta = RecoveryMeta {
        project_path,
        name: project.name.clone(),
        saved_at_ms,
    };

    write_atomic(&rdir, RECOVERY_DOC, project)?;
    write_atomic(&rdir, RECOVERY_META, &meta)?;
    Ok(())
}

/// Serialize `value` to pretty JSON and write it to `<dir>/<name>` atomically via a
/// `<name>.tmp` + `rename`.
fn write_atomic<T: Serialize>(dir: &Path, name: &str, value: &T) -> Result<(), SessionError> {
    let mut json = serde_json::to_string_pretty(value).map_err(json_err)?;
    json.push('\n');
    let tmp = dir.join(format!("{name}.tmp"));
    std::fs::write(&tmp, json).map_err(io_err)?;
    std::fs::rename(&tmp, dir.join(name)).map_err(io_err)
}

/// Read the recovery document + metadata (#63), or `Ok(None)` when there is no
/// recovery file (the common case — nothing to recover). A present-but-corrupt
/// recovery pair is `Ok(None)` too (a broken autosave must never block launch),
/// surfacing an error only for unexpected IO failures.
pub fn read_recovery(dir: &Path) -> Result<Option<Recovery>, SessionError> {
    let rdir = recovery_dir(dir);
    let doc_path = rdir.join(RECOVERY_DOC);
    let meta_path = rdir.join(RECOVERY_META);

    let doc_json = match std::fs::read_to_string(&doc_path) {
        Ok(json) => json,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_err(e)),
    };
    let meta_json = match std::fs::read_to_string(&meta_path) {
        Ok(json) => json,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_err(e)),
    };

    let project: Project = match serde_json::from_str(&doc_json) {
        Ok(p) => p,
        Err(_) => return Ok(None),
    };
    let meta: RecoveryMeta = match serde_json::from_str(&meta_json) {
        Ok(m) => m,
        Err(_) => return Ok(None),
    };
    Ok(Some(Recovery { project, meta }))
}

/// Whether a recovery (with timestamp `recovery_ms`) is **newer** than the last
/// saved version of its project (#63). `last_saved_ms` is the modified-time of the
/// project file in ms, or `None` for a never-saved document (in which case any
/// recovery is "newer" and worth offering). The recovery is only offered when it is
/// strictly newer, so a stale recovery left over from before the last save is not.
pub fn recovery_is_newer(recovery_ms: u64, last_saved_ms: Option<u64>) -> bool {
    match last_saved_ms {
        Some(saved) => recovery_ms > saved,
        None => true,
    }
}

/// Remove the recovery files (#63), e.g. after a successful save or once the user
/// has accepted/declined a restore. A missing recovery dir is a no-op (not an
/// error); an unexpected IO failure is surfaced.
pub fn clear_recovery(dir: &Path) -> Result<(), SessionError> {
    let rdir = recovery_dir(dir);
    match std::fs::remove_dir_all(&rdir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_err(e)),
    }
}

/// The last-saved timestamp (ms since epoch) of the project file at `path`, from
/// its filesystem modified-time, or `None` when the file does not exist / has no
/// readable mtime. Used to decide whether a recovery is newer than the last save.
pub fn file_modified_ms(path: &str) -> Option<u64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recent(path: &str, name: &str) -> RecentProject {
        RecentProject {
            path: path.to_owned(),
            name: name.to_owned(),
        }
    }

    fn sample_project(name: &str) -> Project {
        Project::empty(name)
    }

    #[test]
    fn load_recents_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_recents(dir.path()).is_empty());
    }

    #[test]
    fn push_recent_prepends_dedupes_and_caps() {
        let dir = tempfile::tempdir().unwrap();
        // A real file so load_recents does not prune it.
        let f = dir.path().join("p.json");
        std::fs::write(&f, "{}").unwrap();
        let p = f.to_string_lossy().into_owned();

        push_recent(dir.path(), recent(&p, "First")).unwrap();
        let again = push_recent(dir.path(), recent(&p, "Renamed")).unwrap();
        // De-duped by path: still one entry, with the refreshed name, at the front.
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].name, "Renamed");

        // Cap: push MAX_RECENTS + 5 distinct (non-existent) paths.
        for i in 0..MAX_RECENTS + 5 {
            push_recent(dir.path(), recent(&format!("/np/{i}.json"), "x")).unwrap();
        }
        let raw = read_recents_raw(dir.path());
        assert_eq!(raw.len(), MAX_RECENTS);
        // Most-recent-first: the last pushed path is at the front.
        assert_eq!(raw[0].path, format!("/np/{}.json", MAX_RECENTS + 4));
    }

    #[test]
    fn load_recents_prunes_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.json");
        std::fs::write(&real, "{}").unwrap();
        let real_p = real.to_string_lossy().into_owned();

        push_recent(dir.path(), recent("/gone/missing.json", "Gone")).unwrap();
        push_recent(dir.path(), recent(&real_p, "Real")).unwrap();

        let loaded = load_recents(dir.path());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].path, real_p);
        // The prune was persisted.
        assert_eq!(read_recents_raw(dir.path()).len(), 1);
    }

    #[test]
    fn recovery_round_trips_and_clears() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_recovery(dir.path()).unwrap().is_none());

        let project = sample_project("Recovered Work");
        write_recovery(dir.path(), &project, Some("/x/y.json".to_owned()), 123).unwrap();

        let got = read_recovery(dir.path())
            .unwrap()
            .expect("recovery present");
        assert_eq!(got.project, project);
        assert_eq!(got.meta.project_path.as_deref(), Some("/x/y.json"));
        assert_eq!(got.meta.name, "Recovered Work");
        assert_eq!(got.meta.saved_at_ms, 123);

        clear_recovery(dir.path()).unwrap();
        assert!(read_recovery(dir.path()).unwrap().is_none());
        // Clearing again is a no-op.
        clear_recovery(dir.path()).unwrap();
    }

    #[test]
    fn read_recovery_corrupt_doc_is_none_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let rdir = recovery_dir(dir.path());
        std::fs::create_dir_all(&rdir).unwrap();
        std::fs::write(rdir.join(RECOVERY_DOC), b"{ not json").unwrap();
        std::fs::write(rdir.join(RECOVERY_META), b"{ not json").unwrap();
        assert!(read_recovery(dir.path()).unwrap().is_none());
    }

    #[test]
    fn recovery_is_newer_logic() {
        // Newer than the last save -> offer it.
        assert!(recovery_is_newer(200, Some(100)));
        // Older/equal -> do not offer.
        assert!(!recovery_is_newer(100, Some(100)));
        assert!(!recovery_is_newer(50, Some(100)));
        // Never saved -> always offer.
        assert!(recovery_is_newer(1, None));
    }

    #[test]
    fn file_modified_ms_present_and_absent() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("p.json");
        std::fs::write(&f, "{}").unwrap();
        assert!(file_modified_ms(&f.to_string_lossy()).is_some());
        assert!(file_modified_ms("/definitely/not/here.json").is_none());
    }
}
