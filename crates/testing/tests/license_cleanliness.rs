//! LICENSE CLEANLINESS guard (#67, Decision Log #13): the shipped v1 artifact is
//! OSS (MIT) and must carry NO GPL/LGPL ffmpeg/encoder dependency. ffmpeg is
//! optional and stays out-of-bundle for v1; PNG sequences decode in-core via the
//! `image` crate. This test parses the committed workspace `Cargo.lock` and fails
//! if any forbidden media/encoder crate has crept into the dependency graph, so a
//! future dep that would taint the bundle's license is caught in CI rather than
//! after a release ships.
//!
//! It is a TEXT scan of `Cargo.lock` (no manifest parser dep): the lockfile lists
//! every transitive crate as a `name = "<crate>"` line, so a forbidden crate
//! cannot be present without matching here.

use std::path::{Path, PathBuf};

/// The workspace lockfile: `crates/testing` → repo root `Cargo.lock`.
fn workspace_lock() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("Cargo.lock")
}

/// Crate-name substrings that would pull a GPL/LGPL ffmpeg / hardware-encoder
/// dependency into the bundle. Matched against the `name = "..."` value in
/// `Cargo.lock`. Kept conservative: these are the crates that wrap ffmpeg,
/// gstreamer or proprietary/GPL codecs.
const FORBIDDEN_CRATES: &[&str] = &[
    "ffmpeg",       // ffmpeg-next, ffmpeg-sys, ac-ffmpeg, rsmpeg, ...
    "gstreamer",    // gstreamer / gst-* bindings
    "gst-",         // gstreamer plugin crates
    "x264",         // GPL H.264 encoder
    "x265",         // GPL H.265 encoder
    "libde265",     // GPL HEVC decoder
    "openh264-sys", // (BSD lib but ships under a codec patent grant; keep out of bundle)
];

#[test]
fn cargo_lock_has_no_ffmpeg_or_gpl_codec_dependency() {
    let lock = workspace_lock();
    let text = std::fs::read_to_string(&lock)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", lock.display()));

    // Collect every locked crate name (`name = "foo"`).
    let names: Vec<String> = text
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("name = \"")?;
            let name = rest.strip_suffix('"')?;
            Some(name.to_string())
        })
        .collect();

    assert!(
        !names.is_empty(),
        "parsed zero crate names from {} — lockfile format changed?",
        lock.display()
    );

    let offenders: Vec<&String> = names
        .iter()
        .filter(|name| {
            let lower = name.to_ascii_lowercase();
            FORBIDDEN_CRATES.iter().any(|bad| lower.contains(bad))
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "LICENSE CLEANLINESS (#67): forbidden GPL/LGPL media dependency in Cargo.lock: \
         {offenders:?}. ffmpeg/gstreamer/GPL codecs must stay optional + out-of-bundle \
         for the v1 release (Decision Log #13). Remove the dependency or gate it behind \
         an off-by-default feature that the bundle does not enable."
    );
}
