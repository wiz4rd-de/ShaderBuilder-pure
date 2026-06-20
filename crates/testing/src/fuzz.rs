//! Corpus fuzzer (#32, Architecture §G.2/§G.3): import-and-render a *directory*
//! of `.slangp` presets as a smoke/fidelity fuzzer. Each preset is run through
//! [`crate::render::render_preset_to_image`]; failures are caught **per preset**
//! (a parse/compile/render error — or even a panic deep in the toolchain — on one
//! preset does not abort the run) and reported.
//!
//! This is the "import-and-render a broad slice without crashing" half of the
//! ticket. It only needs a directory of presets:
//!
//! * pointed at the committed [`fixtures`](crate) it is a fast, deterministic CI
//!   smoke test that the engine still imports every feature-exercising fixture;
//! * pointed at a cloned `slang-shaders` checkout it is the **real corpus run**
//!   — driven by the `#[ignore]`d `corpus_fuzz.rs` (keyed off `FUZZ_CORPUS_DIR`),
//!   which prints a categorized compile/render/failure summary. The corpus is a
//!   large external clone and is intentionally NOT vendored here; see
//!   `docs/golden-image-harness.md` §2 for the latest results + failure worklist.

use std::path::{Path, PathBuf};

use source::Frame;

/// The outcome of importing-and-rendering one preset (#32).
#[derive(Debug, Clone)]
pub struct PresetResult {
    /// The preset file's name (the path relative to the walked directory, or its
    /// file name when it is a direct child) — enough to identify it in a report.
    pub name: String,
    /// Whether the `.slangp` parsed AND every pass compiled. `true` even if the
    /// later GPU render failed (so a compile-vs-render regression is
    /// distinguishable).
    pub compiled: bool,
    /// Whether the preset rendered all the way to a read-back image.
    pub rendered: bool,
    /// The first error encountered (parse / compile / LUT / render / panic), or
    /// `None` on success.
    pub error: Option<String>,
}

impl PresetResult {
    /// Whether this preset fully succeeded (compiled and rendered, no error).
    pub fn ok(&self) -> bool {
        self.compiled && self.rendered && self.error.is_none()
    }
}

/// Walk `dir` recursively for `*.slangp` files and import-and-render each over
/// `source` at `viewport`, advanced to `frame_index` (#32). Returns one
/// [`PresetResult`] per preset, sorted by name for a deterministic report. A
/// failure on one preset is recorded and the run continues.
///
/// The walk is shallow-dependency (only `std::fs`); a non-existent or unreadable
/// directory yields an empty `Vec` rather than an error, so a caller can treat a
/// missing optional corpus as "nothing to fuzz".
pub fn fuzz_presets(
    dir: &Path,
    source: &Frame,
    viewport: (u32, u32),
    frame_index: u64,
) -> Vec<PresetResult> {
    let mut presets = Vec::new();
    collect_slangp(dir, &mut presets);
    presets.sort();

    let mut results: Vec<PresetResult> = presets
        .iter()
        .map(|path| run_one(dir, path, source, viewport, frame_index))
        .collect();
    results.sort_by(|a, b| a.name.cmp(&b.name));
    results
}

/// Import-and-render a single preset, catching every failure mode — including a
/// panic deep in the compile toolchain — so the corpus run never aborts.
fn run_one(
    base: &Path,
    path: &Path,
    source: &Frame,
    viewport: (u32, u32),
    frame_index: u64,
) -> PresetResult {
    let name = path
        .strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned();

    // `catch_unwind` guards against a panic in any layer (parser, glslang, naga,
    // wgpu) turning one bad corpus preset into an aborted run. The closure is
    // unwind-safe: it only borrows `&` data and returns an owned result.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::render::render_preset_to_image(path, source, viewport, frame_index)
    }));

    match outcome {
        Ok(Ok(_image)) => PresetResult {
            name,
            compiled: true,
            rendered: true,
            error: None,
        },
        Ok(Err(err)) => {
            // A returned error: `compiled` is true unless the failure was at or
            // before the compile stage (parse / compile / LUT).
            let compiled = matches!(err, crate::render::HarnessError::Render(_));
            PresetResult {
                name,
                compiled,
                rendered: false,
                error: Some(err.to_string()),
            }
        }
        Err(panic) => PresetResult {
            name,
            compiled: false,
            rendered: false,
            error: Some(format!("panicked: {}", panic_message(&panic))),
        },
    }
}

/// Best-effort extraction of a panic payload's message.
fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

/// Recursively collect `*.slangp` files under `dir` into `out`. Unreadable
/// directories are skipped silently (a missing optional corpus is not an error).
fn collect_slangp(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_slangp(&path, out);
        } else if path.extension().is_some_and(|e| e == "slangp") {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_directory_yields_no_results() {
        let src = Frame::new(1, 1, vec![0, 0, 0, 255]);
        let results = fuzz_presets(Path::new("/no/such/corpus/dir"), &src, (2, 2), 0);
        assert!(results.is_empty(), "a missing corpus dir must fuzz nothing");
    }

    #[test]
    fn collects_slangp_recursively() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(dir.path().join("a.slangp"), b"shaders = 0\n").unwrap();
        std::fs::write(sub.join("b.slangp"), b"shaders = 0\n").unwrap();
        std::fs::write(dir.path().join("not_a_preset.txt"), b"x").unwrap();

        let mut found = Vec::new();
        collect_slangp(dir.path(), &mut found);
        found.sort();
        assert_eq!(found.len(), 2, "should find both .slangp, skip the .txt");
        assert!(found.iter().all(|p| p.extension().unwrap() == "slangp"));
    }
}
