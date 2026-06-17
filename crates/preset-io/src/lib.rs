//! `preset-io` — `.slangp`/`.slang` import + export bundle writer (Architecture §B).
//!
//! Phase 1 provides only the minimum the render slice needs: loading a single
//! `.slang` file into the string `slang-compile` consumes, plus the base
//! directory for resolving its `#include`s. Full `.slangp` pipeline
//! reconstruction, parameters, LUTs, and export bundles are Phase 3.

use std::path::{Path, PathBuf};

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
/// reconstruction — that's Phase 3.
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
