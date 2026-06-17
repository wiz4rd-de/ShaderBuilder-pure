//! `preset-io` — `.slangp`/`.slang` import + export bundle writer (Architecture §B).
//!
//! Phase 1 shipped only the minimum the render slice needed: loading a single
//! `.slang` file into the string `slang-compile` consumes. Phase 2 adds the
//! **`.slangp` multi-pass preset parser** ([`parse_slangp`] / [`Preset`]) — the
//! foundation the multi-pass resource graph (#22) and every later Phase-2 ticket
//! (#23 formats/samplers, #24 feedback, #27 LUTs) builds on.
//!
//! The parser is intentionally **forward-looking**: it captures the *full*
//! documented per-pass key set as typed `Option` fields now, even though #22
//! consumes only the scale/shader keys. Later tickets read the already-parsed
//! fields rather than re-touching the parser. See
//! `docs/retroarch-slang-runtime.md` §1 for the authoritative key list and
//! defaults (we follow RetroArch C where it and librashader diverge).

mod slangp;

use std::path::{Path, PathBuf};

pub use slangp::{
    parse_slangp, parse_slangp_str, LutEntry, ParseError, Pass, Preset, ScaleType, WrapMode,
};

/// Crate identity marker (kept from the Phase 0 scaffold so dependent crates'
/// smoke tests keep the dependency edge live).
pub const NAME: &str = "preset-io";

/// A loaded `.slang` source plus the directory to resolve its `#include`s from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlangSource {
    /// The file's contents.
    pub source: String,
    /// Directory of the file, used as the `#include` base (see
    /// `slang_compile::compile_slang`).
    pub base_dir: Option<PathBuf>,
}

/// Read a `.slang` file from disk into a [`SlangSource`]. Returns the file's I/O
/// error (e.g. `NotFound`) if it can't be read. No `.slangp` pipeline
/// reconstruction — use [`parse_slangp`] for that.
pub fn load_slang_file(path: impl AsRef<Path>) -> std::io::Result<SlangSource> {
    let path = path.as_ref();
    let source = std::fs::read_to_string(path)?;
    let base_dir = path.parent().map(Path::to_path_buf);
    Ok(SlangSource { source, base_dir })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn smoke() {
        assert_eq!(NAME, "preset-io");
        assert_eq!(core_model::NAME, "core-model");
    }

    #[test]
    fn loads_a_slang_file_with_base_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.slang");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(b"#version 450\n")
            .unwrap();

        let loaded = load_slang_file(&path).unwrap();
        assert!(loaded.source.contains("#version 450"));
        assert_eq!(loaded.base_dir.as_deref(), Some(dir.path()));
    }

    #[test]
    fn missing_file_is_an_error() {
        let err = load_slang_file("/no/such/file.slang").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }
}
